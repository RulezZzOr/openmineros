//! Chain probe for the Braiins X19 bitstream (S17/S19 Zynq control boards).
//!
//!   omo-chain-probe regs                      read-only register snapshot (JSON)
//!   omo-chain-probe read-reg --chain N [--reg R] [--chip A]
//!                                             send one register read and print the answers
//!
//! `read-reg` writes to the cmd-tx FIFO and pops cmd-rx, so it refuses to run
//! while another miner process (bosminer, cgminer, …) is running. Stop it
//! first, or freeze it with SIGSTOP.

#[cfg(unix)]
fn main() {
    std::process::exit(match imp::run(std::env::args().skip(1).collect()) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("omo-chain-probe: {e}");
            1
        }
    });
}

#[cfg(not(unix))]
fn main() {
    eprintln!("omo-chain-probe runs on the miner only");
    std::process::exit(2);
}

#[cfg(unix)]
mod imp {
    use openmineros_board_xil::x19::{self, Chain};
    use openmineros_board_xil::x19_work::{self as work, MidstateOrder};
    use serde_json::json;
    use std::{
        collections::HashMap,
        fs,
        time::{Duration, Instant},
    };

    const MINERS: [&str; 3] = ["bosminer", "cgminer", "bmminer"];

    pub fn run(args: Vec<String>) -> Result<(), String> {
        let cmd = args.first().map(String::as_str).unwrap_or("help");
        let opt = |name: &str| -> Option<&str> {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
                .map(String::as_str)
        };
        match cmd {
            "regs" => regs(),
            "read-reg" => {
                let chain = parse_u8(opt("--chain").ok_or("--chain N is required")?)?;
                let reg = opt("--reg").map(parse_u8).transpose()?.unwrap_or(0);
                let chip = opt("--chip").map(parse_u8).transpose()?;
                read_reg(chain, chip, reg)
            }
            "fpga-write" => {
                let chain = parse_u8(opt("--chain").ok_or("--chain N is required")?)?;
                let block: x19::Block = opt("--block").unwrap_or("common").parse()?;
                let off = parse_u32(opt("--off").ok_or("--off is required")?)? as usize;
                let value = parse_u32(opt("--value").ok_or("--value is required")?)?;
                with_chain(chain, |c| {
                    c.write_block(block, off, value);
                    println!("{block:?}+{off:#x} <- {value:#010x}, reads {:#010x}", c.read_block(block, off));
                    Ok(())
                })
            }
            "reset-fifos" => {
                let chain = parse_u8(opt("--chain").ok_or("--chain N is required")?)?;
                with_chain(chain, |c| {
                    c.reset_fifos();
                    Ok(())
                })
            }
            "reset" => {
                let chain = parse_u8(opt("--chain").ok_or("--chain N is required")?)?;
                let hold: u64 = opt("--hold-ms").map(str::parse).transpose().map_err(|_| "bad --hold-ms")?.unwrap_or(200);
                let wait: u64 = opt("--wait-ms").map(str::parse).transpose().map_err(|_| "bad --wait-ms")?.unwrap_or(1000);
                guard()?;
                let path = format!("/sys/class/gpio/gpio{}/value", x19::reset_gpio(chain));
                fs::write(&path, "0").map_err(|e| format!("{path}: {e}"))?;
                std::thread::sleep(Duration::from_millis(hold));
                fs::write(&path, "1").map_err(|e| format!("{path}: {e}"))?;
                std::thread::sleep(Duration::from_millis(wait));
                println!("chain {chain} reset pulse {hold} ms, waited {wait} ms");
                Ok(())
            }
            "write-reg" => {
                let chain = parse_u8(opt("--chain").ok_or("--chain N is required")?)?;
                let reg = parse_u8(opt("--reg").ok_or("--reg is required")?)?;
                let value = parse_u32(opt("--value").ok_or("--value is required")?)?;
                let chip = opt("--chip").map(parse_u8).transpose()?;
                with_chain(chain, |c| {
                    c.send(&x19::write_reg(chip, reg, value)).map_err(|e| e.to_string())?;
                    std::thread::sleep(Duration::from_millis(5));
                    Ok(())
                })
            }
            "chain-inactive" => {
                let chain = parse_u8(opt("--chain").ok_or("--chain N is required")?)?;
                with_chain(chain, |c| {
                    for _ in 0..3 {
                        c.send(&x19::chain_inactive()).map_err(|e| e.to_string())?;
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    Ok(())
                })
            }
            "set-addr" => {
                let chain = parse_u8(opt("--chain").ok_or("--chain N is required")?)?;
                let count = parse_u8(opt("--count").ok_or("--count is required")?)?;
                let step = parse_u8(opt("--step").unwrap_or("3"))?;
                with_chain(chain, |c| {
                    for i in 0..count {
                        c.send(&x19::set_address(i.wrapping_mul(step))).map_err(|e| e.to_string())?;
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    println!("assigned {count} addresses, step {step}");
                    Ok(())
                })
            }
            "dump-regs" => {
                let chain = parse_u8(opt("--chain").ok_or("--chain N is required")?)?;
                let regs = parse_list(opt("--regs").ok_or("--regs is required")?)?;
                with_chain(chain, |c| {
                    let mut out = serde_json::Map::new();
                    for reg in regs {
                        out.insert(format!("{reg:#04x}"), serde_json::to_value(read_all(c, reg)?).unwrap());
                    }
                    println!("{}", serde_json::to_string_pretty(&out).unwrap());
                    Ok(())
                })
            }
            "apply-regs" => {
                let chain = parse_u8(opt("--chain").ok_or("--chain N is required")?)?;
                let file = opt("--file").ok_or("--file is required")?;
                let regs = parse_list(opt("--regs").ok_or("--regs is required")?)?;
                let keep_low16: Vec<u8> = opt("--keep-low16").map(parse_list).transpose()?.unwrap_or_default();
                let text = fs::read_to_string(file).map_err(|e| format!("{file}: {e}"))?;
                let dump: HashMap<String, HashMap<String, String>> =
                    serde_json::from_str(&text).map_err(|e| e.to_string())?;
                with_chain(chain, |c| apply_regs(c, &dump, &regs, &keep_low16))
            }
            "apw" => {
                // omo-chain-probe apw --cmd type|fw|dac      (queries)
                // omo-chain-probe apw --set-dac N            (sets PSU output DAC)
                use openmineros_board_xil::{bitbang::BitBangI2c, i2c::apw};
                let bus = BitBangI2c::open(1050).map_err(|e| e.to_string())?;
                let (cmd, payload) = if let Some(d) = opt("--set-dac") {
                    let d = parse_u8(d)?;
                    if !(apw::DAC_MIN..=apw::DAC_MAX).contains(&d) {
                        return Err(format!("DAC {d:#04x} outside {:#04x}..={:#04x}", apw::DAC_MIN, apw::DAC_MAX));
                    }
                    (apw::SET_DAC, vec![d, 0])
                } else {
                    match opt("--cmd").unwrap_or("type") {
                        "type" => (apw::DEVICE_TYPE, vec![]),
                        "fw" => (apw::FW_VERSION, vec![]),
                        "dac" => (apw::GET_DAC, vec![]),
                        other => return Err(format!("unknown --cmd {other}")),
                    }
                };
                let reply = bus.apw_exchange(apw::ADDR, &apw::frame(cmd, &payload))?;
                let hex = reply.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ");
                match apw::parse(cmd, &reply) {
                    Ok(p) => {
                        let v = p.first().copied().unwrap_or(0);
                        let volts = if cmd == apw::GET_DAC || cmd == apw::SET_DAC {
                            format!(" (~{:.2} V)", apw::volts_for_dac(v))
                        } else {
                            String::new()
                        };
                        println!("cmd {cmd:#04x}: payload {p:02x?} raw {hex}{volts}");
                        Ok(())
                    }
                    Err(e) => Err(format!("{e} (raw {hex})")),
                }
            }
            "sniff" => {
                // omo-chain-probe sniff --seconds 20 --out /tmp/psu.sniff   (read-only)
                let secs: u64 = opt("--seconds").map(str::parse).transpose().map_err(|_| "bad --seconds")?.unwrap_or(10);
                let out = opt("--out").unwrap_or("/tmp/psu.sniff");
                let (ev, n, el) = openmineros_board_xil::bitbang::sniff(Duration::from_secs(secs), 1).map_err(|e| e.to_string())?;
                let mut text = format!("# samples={n} elapsed_us={} events={}\n", el.as_micros(), ev.len());
                for (i, v) in &ev {
                    text.push_str(&format!("{i} {v}\n"));
                }
                fs::write(out, text).map_err(|e| e.to_string())?;
                println!("samples={n} rate={:.2} MS/s events={}", n as f64 / el.as_secs_f64() / 1e6, ev.len());
                Ok(())
            }
            "pic" => {
                // omo-chain-probe pic --addrs 0x20,0x21 --cmd version|heartbeat [--bus 0]
                let addrs = parse_list(opt("--addrs").ok_or("--addrs is required")?)?;
                let bus = parse_u8(opt("--bus").unwrap_or("0"))?;
                let cmd = match opt("--cmd").unwrap_or("version") {
                    "version" => openmineros_board_xil::i2c::pic::GET_VERSION,
                    "heartbeat" => openmineros_board_xil::i2c::pic::HEARTBEAT,
                    other => return Err(format!("unknown --cmd {other}")),
                };
                let frame = openmineros_board_xil::i2c::pic::frame(cmd, &[]);
                let mut dev = openmineros_board_xil::i2c::I2cDev::open(bus).map_err(|e| e.to_string())?;
                for a in addrs {
                    match dev.pic_transaction(a, &frame, 6) {
                        Ok(r) => println!("{a:#04x}: {}", r.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")),
                        Err(e) => println!("{a:#04x}: error {e}"),
                    }
                }
                Ok(())
            }
            "work-test" => {
                let chain = parse_u8(opt("--chain").ok_or("--chain N is required")?)?;
                let ms: u64 = opt("--ms").map(str::parse).transpose().map_err(|_| "bad --ms")?.unwrap_or(1500);
                let slots: usize = opt("--slots").map(str::parse).transpose().map_err(|_| "bad --slots")?.unwrap_or(8);
                let shift: u32 = opt("--id-shift").map(str::parse).transpose().map_err(|_| "bad --id-shift")?.unwrap_or(3);
                let orders: Vec<MidstateOrder> = match opt("--order") {
                    None => MidstateOrder::ALL.to_vec(),
                    Some(name) => vec![MidstateOrder::ALL
                        .into_iter()
                        .find(|o| format!("{o:?}").eq_ignore_ascii_case(name))
                        .ok_or("unknown --order")?],
                };
                work_test(chain, ms, slots, shift, &orders)
            }
            _ => {
                Err("usage: omo-chain-probe regs | read-reg --chain N [--reg R] [--chip A] | work-test --chain N [--ms 1500] [--slots 8] [--id-shift 3]".into())
            }
        }
    }

    fn parse_u32(s: &str) -> Result<u32, String> {
        let r = match s.strip_prefix("0x") {
            Some(h) => u32::from_str_radix(h, 16),
            None => s.parse(),
        };
        r.map_err(|_| format!("not a number: {s}"))
    }

    fn guard() -> Result<(), String> {
        let busy = active_miners();
        if busy.is_empty() {
            Ok(())
        } else {
            Err(format!(
                "another miner owns the chain FIFOs: {}",
                busy.join(", ")
            ))
        }
    }

    fn with_chain(index: u8, f: impl FnOnce(&Chain) -> Result<(), String>) -> Result<(), String> {
        guard()?;
        let chain = Chain::open(index).map_err(|e| format!("chain {index}: {e}"))?;
        if chain.build_id() != x19::BUILD_ID_AM2 {
            return Err(format!(
                "unknown bitstream BUILD_ID {:#010x}",
                chain.build_id()
            ));
        }
        f(&chain)
    }

    fn parse_u8(s: &str) -> Result<u8, String> {
        let r = match s.strip_prefix("0x") {
            Some(h) => u8::from_str_radix(h, 16),
            None => s.parse(),
        };
        r.map_err(|_| format!("not a byte: {s}"))
    }

    fn regs() -> Result<(), String> {
        let mut chains = Vec::new();
        for n in 1..=4 {
            match Chain::open(n) {
                Ok(c) => chains.push(serde_json::to_value(c.snapshot()).unwrap()),
                Err(e) => chains.push(json!({ "chain": n, "error": e.to_string() })),
            }
        }
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "chains": chains })).unwrap()
        );
        Ok(())
    }

    /// Returns running (not stopped) miner processes as `name:pid:state`.
    fn active_miners() -> Vec<String> {
        let mut out = Vec::new();
        let Ok(dir) = fs::read_dir("/proc") else {
            return out;
        };
        for e in dir.flatten() {
            let pid = e.file_name().to_string_lossy().into_owned();
            if !pid.bytes().all(|b| b.is_ascii_digit()) {
                continue;
            }
            let Ok(stat) = fs::read_to_string(e.path().join("stat")) else {
                continue;
            };
            let (Some(l), Some(r)) = (stat.find('('), stat.rfind(')')) else {
                continue;
            };
            let name = &stat[l + 1..r];
            let state = stat[r + 1..].split_whitespace().next().unwrap_or("?");
            if MINERS.contains(&name) && state != "T" && state != "t" {
                out.push(format!("{name}:{pid}:{state}"));
            }
        }
        out
    }

    fn parse_list(s: &str) -> Result<Vec<u8>, String> {
        s.split(',')
            .filter(|x| !x.is_empty())
            .map(parse_u8)
            .collect()
    }

    /// Broadcast-reads `reg` and returns chip address -> value (hex strings).
    fn read_all(c: &Chain, reg: u8) -> Result<std::collections::BTreeMap<String, String>, String> {
        c.drain_cmd_rx();
        c.send(&x19::read_reg(None, reg))
            .map_err(|e| e.to_string())?;
        let words = c.collect(Duration::from_millis(100), Duration::from_millis(2000));
        Ok(x19::decode_responses(&words)
            .into_iter()
            .filter(|a| a.reg == reg)
            .map(|a| (a.chip.to_string(), format!("{:#010x}", a.value)))
            .collect())
    }

    fn hex_u32(s: &str) -> Result<u32, String> {
        u32::from_str_radix(s.trim_start_matches("0x"), 16).map_err(|_| format!("bad value {s}"))
    }

    /// Writes saved register values back: one broadcast when every chip had
    /// the same value, otherwise one write per chip. For registers in
    /// `keep_low16`, the low 16 bits are taken from the chip's current value
    /// (MiscControl holds the UART divider there). PLL0 (`0x08`) is written
    /// without its lock flag (bit 31), twice, as the vendor code does.
    fn apply_regs(
        c: &Chain,
        dump: &HashMap<String, HashMap<String, String>>,
        regs: &[u8],
        keep_low16: &[u8],
    ) -> Result<(), String> {
        for &reg in regs {
            let key = format!("{reg:#04x}");
            let Some(saved) = dump.get(&key) else {
                println!("{key}: not in dump, skipped");
                continue;
            };
            let current = if keep_low16.contains(&reg) {
                read_all(c, reg)?
            } else {
                Default::default()
            };
            let mut values: Vec<(u8, u32)> = Vec::new();
            for (chip, v) in saved {
                let chip: u8 = chip.parse().map_err(|_| format!("bad chip {chip}"))?;
                let mut v = hex_u32(v)?;
                if reg == 0x08 {
                    v &= 0x7FFF_FFFF;
                }
                if let Some(cur) = current.get(&chip.to_string()) {
                    v = (v & 0xFFFF_0000) | (hex_u32(cur)? & 0xFFFF);
                }
                values.push((chip, v));
            }
            values.sort_unstable();
            let first = values.first().map(|x| x.1);
            let uniform = values.iter().all(|x| Some(x.1) == first);
            let repeat = if reg == 0x08 { 2 } else { 1 };
            for _ in 0..repeat {
                if uniform {
                    if let Some(v) = first {
                        c.send(&x19::write_reg(None, reg, v))
                            .map_err(|e| e.to_string())?;
                    }
                } else {
                    for (chip, v) in &values {
                        c.send(&x19::write_reg(Some(*chip), reg, *v))
                            .map_err(|e| e.to_string())?;
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            println!(
                "{key}: {} {}",
                if uniform { "broadcast" } else { "per-chip" },
                first.map(|v| format!("{v:#010x}")).unwrap_or_default()
            );
        }
        Ok(())
    }

    fn read_reg(index: u8, chip: Option<u8>, reg: u8) -> Result<(), String> {
        let busy = active_miners();
        if !busy.is_empty() {
            return Err(format!(
                "another miner owns the chain FIFOs: {}",
                busy.join(", ")
            ));
        }
        let chain = Chain::open(index).map_err(|e| format!("chain {index}: {e}"))?;
        if chain.build_id() != x19::BUILD_ID_AM2 {
            return Err(format!(
                "unknown bitstream BUILD_ID {:#010x}",
                chain.build_id()
            ));
        }
        let before = chain.snapshot();
        let stale = chain.drain_cmd_rx();
        let command = x19::read_reg(chip, reg);
        chain.send(&command).map_err(|e| e.to_string())?;
        let words = chain.collect(Duration::from_millis(100), Duration::from_millis(2000));
        let answers = x19::decode_responses(&words);
        let mut chips: Vec<u8> = answers.iter().map(|a| a.chip).collect();
        chips.sort_unstable();
        chips.dedup();
        let mut values: Vec<String> = answers
            .iter()
            .map(|a| format!("{:#010x}", a.value))
            .collect();
        values.sort();
        values.dedup();
        let report = json!({
            "chain": index,
            "before": before,
            "stale_words_dropped": stale.len(),
            "command": command.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "),
            "raw_words": words.iter().map(|w| format!("{w:08x}")).collect::<Vec<_>>(),
            "responses": answers.len(),
            "distinct_chip_addresses": chips.len(),
            "first": answers.iter().take(4).collect::<Vec<_>>(),
            "values": values,
            "after": chain.snapshot(),
        });
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        Ok(())
    }

    /// Feeds synthetic work in each midstate word order and checks every
    /// returned nonce against SHA256d. Valid nonces have >= 32 zero bits.
    fn work_test(
        index: u8,
        ms: u64,
        slots: usize,
        shift: u32,
        orders: &[MidstateOrder],
    ) -> Result<(), String> {
        let busy = active_miners();
        if !busy.is_empty() {
            return Err(format!(
                "another miner owns the chain FIFOs: {}",
                busy.join(", ")
            ));
        }
        let chain = Chain::open(index).map_err(|e| format!("chain {index}: {e}"))?;
        if chain.build_id() != x19::BUILD_ID_AM2 {
            return Err(format!(
                "unknown bitstream BUILD_ID {:#010x}",
                chain.build_id()
            ));
        }
        let mut stale = Vec::new();
        while let Some((w0, w1)) = chain.pop_nonce() {
            if stale.len() < 2000 {
                stale.push(work::decode_nonce(w0, w1));
            }
        }
        chain.init_work_tx_threshold();
        let mut results = Vec::new();
        for (v, &order) in orders.iter().enumerate() {
            let base = (v as u32 + 1) * 1000;
            let mut sent: HashMap<u32, [u8; 80]> = HashMap::new();
            let mut nonces = Vec::new();
            let mut n = 0u32;
            let start = Instant::now();
            let mut next_push = Instant::now();
            while start.elapsed() < Duration::from_millis(ms) {
                if Instant::now() >= next_push {
                    let job = base + n;
                    let header = work::test_header(job);
                    if chain.push_work(&work::frame(job << shift, &header, slots, order)) {
                        sent.insert(job, header);
                        n += 1;
                    }
                    next_push += Duration::from_micros(3000);
                }
                while let Some((w0, w1)) = chain.pop_nonce() {
                    nonces.push(work::decode_nonce(w0, w1));
                }
                std::thread::sleep(Duration::from_micros(200));
            }
            // Let the last answers arrive.
            let tail = Instant::now();
            while tail.elapsed() < Duration::from_millis(100) {
                while let Some((w0, w1)) = chain.pop_nonce() {
                    nonces.push(work::decode_nonce(w0, w1));
                }
                std::thread::sleep(Duration::from_millis(1));
            }
            let mut valid = 0;
            let mut matched = 0;
            let mut best = 0;
            let mut zero_hist = [0u32; 8];
            // For every nonce, find the sent job it is really valid for
            // (>= 32 zero bits) and record the offset to the reported job.
            let mut offsets: std::collections::BTreeMap<i64, u32> = Default::default();
            let mut orphans = 0;
            for nw in &nonces {
                let job = u32::from(nw.work_id) >> shift;
                if let Some(h) = sent.get(&job) {
                    matched += 1;
                    let z = work::hash_zero_bits(h, nw.nonce);
                    best = best.max(z);
                    zero_hist[(z / 8).min(7) as usize] += 1;
                    if z >= 32 {
                        valid += 1;
                    }
                }
                let real = sent
                    .iter()
                    .find(|(_, h)| work::hash_zero_bits(h, nw.nonce) >= 32)
                    .map(|(j, _)| *j);
                match real {
                    Some(j) => *offsets.entry(i64::from(j) - i64::from(job)).or_default() += 1,
                    None => orphans += 1,
                }
            }
            results.push(json!({
                "order": order,
                "frames_sent": n,
                "nonces": nonces.len(),
                "matched_work_id": matched,
                "valid_diff1": valid,
                "true_job_offset_hist": offsets.iter().map(|(k, v)| format!("{k}:{v}")).collect::<Vec<_>>(),
                "no_job_found": orphans,
                "all": nonces.iter().map(|x| format!("{:08x}:{:02x}:{:04x}:{:02x}", x.nonce, x.solution, x.work_id, x.crc)).collect::<Vec<_>>(),
                "best_zero_bits": best,
                "zero_bits_hist_by_8": zero_hist,
                "sample": nonces.iter().take(6).map(|x| format!("{:08x} sol={} id={:#x} crc={:#04x}", x.nonce, x.solution, x.work_id, x.crc)).collect::<Vec<_>>(),
            }));
        }
        let report = json!({
            "chain": index,
            "slots": slots,
            "id_shift": shift,
            "stale_nonces": stale.len(),
            "stale_sample": stale.iter().take(6).map(|x| format!("{:08x} sol={} id={:#x} crc={:#04x}", x.nonce, x.solution, x.work_id, x.crc)).collect::<Vec<_>>(),
            "variants": results,
        });
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        Ok(())
    }
}
