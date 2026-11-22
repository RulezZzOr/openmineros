//! OpenMinerOS miner daemon for a plain Antminer S19 on the X19 control board.
//!
//!   omo-minerd [--config /data/omo/config.json] [--seconds N]
//!
//! It owns the whole machine: fans, PSU, hashboard voltage controllers,
//! chain bring-up, frequency/voltage (OC) and Stratum mining with pool
//! failover. Live status goes to /tmp/omo-status.json. On exit
//! (SIGTERM/SIGINT, overheat, fatal error) it switches boards and PSU off.

#[cfg(unix)]
fn main() {
    std::process::exit(match imp::run() {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("omo-minerd: {e}");
            1
        }
    });
}

#[cfg(not(unix))]
fn main() {
    eprintln!("omo-minerd runs on the miner only");
    std::process::exit(2);
}

#[cfg(unix)]
mod imp {
    use openmineros_board_xil::{
        i2c::apw,
        x19::{self, Chain},
        x19_board::{self as board, ChainConfig, Fans, Pics, Psu},
        x19_mine::{FanControl, MineOptions, mine},
    };
    use openmineros_common::miner_config::{CONFIG_PATH, FanMode, MinerConfig, STATUS_PATH};
    use serde_json::json;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        thread,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    static STOP: AtomicBool = AtomicBool::new(false);

    extern "C" fn on_signal(_: libc::c_int) {
        STOP.store(true, Ordering::Relaxed);
    }

    fn now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    fn log(msg: &str) {
        eprintln!("[{}] {msg}", now());
    }

    fn status(state: &str, msg: &str, extra: &serde_json::Value) {
        let mut v = json!({ "state": state, "message": msg, "updated": now() });
        if let (Some(o), Some(e)) = (v.as_object_mut(), extra.as_object()) {
            for (k, x) in e {
                o.insert(k.clone(), x.clone());
            }
        }
        let tmp = format!("{STATUS_PATH}.tmp");
        if std::fs::write(&tmp, v.to_string()).is_ok() {
            let _ = std::fs::rename(&tmp, STATUS_PATH);
        }
    }

    fn other_miner_running() -> bool {
        std::fs::read_dir("/proc")
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| {
                std::fs::read_to_string(e.path().join("comm"))
                    .map(|c| matches!(c.trim(), "bosminer" | "cgminer" | "bmminer"))
                    .unwrap_or(false)
            })
    }

    fn arg(name: &str) -> Option<String> {
        let a: Vec<String> = std::env::args().collect();
        a.iter()
            .position(|x| x == name)
            .and_then(|i| a.get(i + 1))
            .cloned()
    }

    fn sleep_unless_stopped(d: Duration) {
        let end = std::time::Instant::now() + d;
        while std::time::Instant::now() < end && !STOP.load(Ordering::Relaxed) {
            thread::sleep(Duration::from_millis(200));
        }
    }

    fn keepalive(pics: &mut Pics, chains: &[u8]) {
        for &m in chains {
            let _ = pics.heartbeat(board::pic_addr(m));
        }
    }

    pub fn run() -> Result<(), String> {
        let path = arg("--config").unwrap_or_else(|| CONFIG_PATH.to_owned());
        let seconds: u64 = arg("--seconds").and_then(|s| s.parse().ok()).unwrap_or(0);
        let cfg = match MinerConfig::load(&path) {
            Ok(c) => c,
            Err(e) => {
                status(
                    "unconfigured",
                    &format!("no valid configuration: {e}"),
                    &json!({}),
                );
                return Err(e);
            }
        };
        if other_miner_running() {
            return Err("another miner (bosminer/cgminer) is running; stop it first".into());
        }
        // SAFETY: the handler only stores to an atomic.
        unsafe {
            libc::signal(libc::SIGTERM, on_signal as *const () as libc::sighandler_t);
            libc::signal(libc::SIGINT, on_signal as *const () as libc::sighandler_t);
        }

        let (pll_target, freq) = board::pll_for_freq(cfg.freq_mhz);
        let dac_target = apw::dac_for_volts(cfg.voltage);
        // Bring-up at DAC 0xd8 (~12.26 V) unless the target is higher.
        let init_dac: u8 = 0xd8.min(dac_target);
        let extra = json!({
            "freq_mhz": freq, "pll0": format!("{pll_target:#010x}"),
            "voltage": apw::volts_for_dac(dac_target), "dac": dac_target,
            "max_pcb_temp": cfg.max_pcb_temp, "started": now(),
        });

        let chains: Vec<u8> = (1..=3).filter(|&n| board::board_present(n)).collect();
        log(&format!(
            "boards {chains:?}, target {freq:.1} MHz ({pll_target:#010x}), {:.2} V (DAC {dac_target:#04x})",
            apw::volts_for_dac(dac_target)
        ));
        status("starting", "detecting hardware", &extra);
        if chains.is_empty() {
            status("error", "no hashboard detected", &extra);
            return Err("no hashboard detected".into());
        }

        board::set_board_control_c52()?;
        let fans = Fans::open()?;
        fans.set_pwm(70);
        let mut pics = Pics::open()?;
        for &n in &chains {
            let v = pics.bring_up(board::pic_addr(n))?;
            log(&format!("chain {n}: voltage controller {v:#04x}"));
        }
        let psu = Psu::open()?;
        let kind = psu.device_type()?;
        if kind != 0x71 {
            return Err(format!(
                "unexpected PSU type {kind:#06x} (expected APW121215a 0x0071)"
            ));
        }
        psu.set_dac(init_dac)?;
        status("starting", "powering hashboards", &extra);
        psu.enable(true)?;
        thread::sleep(Duration::from_secs(2));

        let result = (|| -> Result<(), String> {
            for &n in &chains {
                pics.enable_voltage(board::pic_addr(n), true)?;
                thread::sleep(Duration::from_millis(500));
            }
            thread::sleep(Duration::from_secs(2));
            // Bring-up always at the measured-good 109 MHz, then ramp.
            let cfg_init = ChainConfig {
                pll0: board::PLL0_109MHZ,
            };
            let mut opened = Vec::new();
            for &n in &chains {
                keepalive(&mut pics, &chains);
                status("starting", &format!("initialising chain {n}"), &extra);
                let c = Chain::open(n).map_err(|e| format!("chain {n}: {e}"))?;
                if c.build_id() != x19::BUILD_ID_AM2 {
                    return Err(format!("chain {n}: unknown FPGA bitstream"));
                }
                let found = board::cold_init(&c, &cfg_init)?;
                log(&format!("chain {n}: {found} chips"));
                if found < usize::from(board::CHIPS) - 6 {
                    return Err(format!("chain {n}: only {found} chips answer"));
                }
                opened.push(c);
            }
            // Voltage goes up before the clock and down after it.
            let mut cur_dac = init_dac;
            let mut cur_f = board::pll_freq_mhz(board::PLL0_109MHZ);
            let ramp_voltage = |from: u8, to: u8, pics: &mut Pics| -> Result<(), String> {
                let mut d = from;
                while d != to {
                    d = if d < to {
                        d.saturating_add(8).min(to)
                    } else {
                        d.saturating_sub(8).max(to)
                    };
                    psu.set_dac(d)?;
                    keepalive(pics, &chains);
                }
                Ok(())
            };
            if dac_target < cur_dac {
                status("starting", "raising voltage", &extra);
                ramp_voltage(cur_dac, dac_target, &mut pics)?;
                cur_dac = dac_target;
            }
            while (cur_f - freq).abs() > 0.5 {
                let next = if freq > cur_f {
                    (cur_f + 25.0).min(freq)
                } else {
                    (cur_f - 25.0).max(freq)
                };
                let (pll, f) = if (next - freq).abs() < 0.5 {
                    (pll_target, freq)
                } else {
                    board::pll_for_freq(next)
                };
                for c in &opened {
                    board::set_frequency(c, pll)?;
                }
                keepalive(&mut pics, &chains);
                cur_f = f;
                status("starting", &format!("ramping clock {cur_f:.0} MHz"), &extra);
                thread::sleep(Duration::from_millis(300));
            }
            if dac_target > cur_dac {
                ramp_voltage(cur_dac, dac_target, &mut pics)?;
            }
            log(&format!(
                "chains at {cur_f:.1} MHz, {:.2} V",
                apw::volts_for_dac(dac_target)
            ));

            let pic_addrs: Vec<u8> = chains.iter().map(|&n| board::pic_addr(n)).collect();
            let stop = Arc::new(AtomicBool::new(false));
            let s2 = stop.clone();
            thread::spawn(move || {
                while !STOP.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(200));
                }
                s2.store(true, Ordering::Relaxed);
            });
            let fan = FanControl {
                auto: cfg.fan.mode == FanMode::Auto,
                target_temp: cfg.fan.target_temp,
                min_percent: cfg.fan.min_percent,
                manual_percent: cfg.fan.manual_percent,
            };
            // Pool failover: pools in order, a pool error moves to the next.
            let mut i = 0usize;
            let mut failures = 0usize;
            while !STOP.load(Ordering::Relaxed) {
                let idx = i % cfg.pools.len();
                let p = &cfg.pools[idx];
                log(&format!("pool {}: {}", idx + 1, p.host_port()));
                status("connecting", &format!("pool {}", p.host_port()), &extra);
                let opts = MineOptions {
                    host: p.host_port(),
                    user: p.user.clone(),
                    password: p.password.clone(),
                    seconds,
                    frame_interval: Duration::from_micros(1500),
                    pic_heartbeat: pic_addrs.clone(),
                    max_pcb_temp: cfg.max_pcb_temp,
                    fan: fan.clone(),
                    stop: stop.clone(),
                    status_path: Some(STATUS_PATH.to_owned()),
                    best_path: Some("/data/omo/best.json".to_owned()),
                    status_extra: json!({
                        "freq_mhz": cur_f, "voltage": apw::volts_for_dac(dac_target), "dac": dac_target,
                        "pool_index": idx, "max_pcb_temp": cfg.max_pcb_temp, "started": extra["started"],
                    }),
                };
                match mine(&opened, &opts) {
                    Ok(rep) => {
                        log(&format!("mining ended: {}", rep.stop_reason));
                        break;
                    }
                    Err(e) if e.starts_with("overheat") || e.contains("voltage controllers") => {
                        return Err(e);
                    }
                    Err(e) => {
                        log(&format!("pool error: {e}"));
                        failures += 1;
                        i += 1;
                        let wait = if failures % cfg.pools.len() == 0 {
                            10
                        } else {
                            1
                        };
                        for _ in 0..wait {
                            keepalive(&mut pics, &chains);
                            sleep_unless_stopped(Duration::from_secs(1));
                        }
                    }
                }
            }
            Ok(())
        })();

        log("shutting down: board voltage off, PSU off");
        if let Ok(mut p) = Pics::open() {
            for &n in &chains {
                let _ = p.enable_voltage(board::pic_addr(n), false);
            }
        }
        let _ = psu.enable(false);
        fans.set_pwm(30);
        match &result {
            Ok(()) => status("stopped", "stopped", &extra),
            Err(e) => status("error", e, &extra),
        }
        result
    }
}
