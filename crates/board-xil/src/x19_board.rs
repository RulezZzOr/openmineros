//! Whole-board bring-up for a plain Antminer S19 (BM1398, 3×76 chips) on the
//! X19 control board, without bosminer. Every sequence and value here was
//! recorded from bosminer on `.47` (I2C syscall trace, PSU bus capture,
//! chip register dumps), see `docs/research/braiins-x19-fifo-live-47.md`.

use crate::i2c::{apw, pic};

/// Chips per chain and address step on a plain S19.
pub const CHIPS: u8 = 76;
pub const ADDR_STEP: u8 = 3;

/// PLL0 for 109 MHz (`25 * 218 / (2 * 5 * 5)`), as bosminer tuned it.
pub const PLL0_109MHZ: u32 = 0x40DA_0255;

/// UART relay register 0x2c: chips at addresses 15, 33, …, 213 relay the
/// UART between voltage domains; values recorded on board 1.
pub fn relay_value(addr: u8) -> u32 {
    if addr >= 15 && (addr - 15) % 18 == 0 && addr <= 213 {
        let k = u32::from((addr - 15) / 18);
        ((0x56 - 6 * k) << 16) | 0x0003
    } else {
        0x000F_0000
    }
}

/// MiscControl 0x18 with bosminer's fast-UART setting (two chips differ).
pub fn misc_control(addr: u8) -> u32 {
    if addr == 57 || addr == 165 {
        0xF001_8005
    } else {
        0xF001_A005
    }
}

/// Hash-counting register 0x10 scales with 1/frequency (bosminer: 0xEEC0 at
/// ~72 MHz, 0x9EF0 at 109 MHz).
pub fn hash_counting(freq_mhz: f64) -> u32 {
    (4_435_000.0 / freq_mhz).round() as u32
}

/// FPGA WORK_TIME (bosminer: 0x3FAA8 at 109 MHz, 0x5F805 at 80 MHz).
pub fn work_time(freq_mhz: f64) -> u32 {
    (28_400_000.0 / freq_mhz).round() as u32
}

/// Finds the PLL0 value closest to `mhz` (VCO 2000–3200 MHz from the 25 MHz
/// reference, refdiv 1 or 2, post dividers 1–7). Returns (PLL0, real MHz).
pub fn pll_for_freq(mhz: f64) -> (u32, f64) {
    let mut best = (0u32, 0.0f64, f64::MAX);
    for refdiv in [2u32, 1] {
        for fb in 16..=250u32 {
            let vco = 25.0 * f64::from(fb) / f64::from(refdiv);
            if !(2000.0..=3200.0).contains(&vco) {
                continue;
            }
            for p1 in 1..=7u32 {
                for p2 in 1..=p1 {
                    let f = vco / f64::from(p1 * p2);
                    let err = (f - mhz).abs();
                    if err < best.2 - 1e-9 {
                        best = (
                            (1 << 30) | (fb << 16) | (refdiv << 8) | (p1 << 4) | p2,
                            f,
                            err,
                        );
                    }
                }
            }
        }
    }
    (best.0, best.1)
}

pub fn pll_freq_mhz(pll0: u32) -> f64 {
    let fb = f64::from((pll0 >> 16) & 0xFFF);
    let rd = f64::from((pll0 >> 8) & 0x3F);
    let p1 = f64::from((pll0 >> 4) & 7);
    let p2 = f64::from(pll0 & 7);
    25.0 * fb / (rd * p1 * p2)
}

#[cfg(unix)]
pub use hw::*;

#[cfg(unix)]
mod hw {
    use super::*;
    use crate::bitbang::BitBangI2c;
    use crate::i2c::I2cDev;
    use crate::uio::UioDevice;
    use crate::x19::{self, Block, Chain};
    use std::{fs, path::Path, thread, time::Duration};

    fn ms(n: u64) {
        thread::sleep(Duration::from_millis(n));
    }

    fn gpio_set(n: u32, v: bool) -> Result<(), String> {
        let dir = format!("/sys/class/gpio/gpio{n}");
        if !Path::new(&dir).exists() {
            fs::write("/sys/class/gpio/export", n.to_string())
                .map_err(|e| format!("export {n}: {e}"))?;
            ms(50);
        }
        fs::write(format!("{dir}/direction"), if v { "high" } else { "low" })
            .map_err(|e| format!("gpio{n}: {e}"))
    }

    pub fn gpio_get(n: u32) -> Option<bool> {
        let dir = format!("/sys/class/gpio/gpio{n}");
        if !Path::new(&dir).exists() {
            let _ = fs::write("/sys/class/gpio/export", n.to_string());
            ms(50);
            let _ = fs::write(format!("{dir}/direction"), "in");
        }
        fs::read_to_string(format!("{dir}/value"))
            .ok()
            .map(|s| s.trim() == "1")
    }

    /// Hashboard present on chain `n` (plug detect gpio 902..904, high = present).
    pub fn board_present(n: u8) -> bool {
        gpio_get(901 + u32::from(n)).unwrap_or(false)
    }

    // ---------------------------------------------------------------- fans

    pub struct Fans {
        dev: UioDevice,
    }

    impl Fans {
        pub fn open() -> Result<Self, String> {
            Ok(Self {
                dev: UioDevice::open_by_name("fan-control").map_err(|e| e.to_string())?,
            })
        }

        /// Sets both PWM channels (percent, as bosminer writes them).
        pub fn set_pwm(&self, percent: u32) {
            let p = percent.min(100);
            self.dev.write(0x10, p);
            self.dev.write(0x14, p);
        }

        /// Raw tachometer values of the four fans.
        pub fn tach(&self) -> [u32; 4] {
            [
                self.dev.read(0x00),
                self.dev.read(0x04),
                self.dev.read(0x08),
                self.dev.read(0x0c),
            ]
        }
    }

    /// Board-control mode C52 (2 PWM fan outputs), as bosminer configures it.
    pub fn set_board_control_c52() -> Result<(), String> {
        let d = UioDevice::open_by_name("board-control").map_err(|e| e.to_string())?;
        d.write(0x04, 0x34);
        Ok(())
    }

    // ----------------------------------------------------------------- PSU

    pub struct Psu {
        bus: BitBangI2c,
    }

    impl Psu {
        pub fn open() -> Result<Self, String> {
            Ok(Self {
                bus: BitBangI2c::open(1050).map_err(|e| e.to_string())?,
            })
        }

        fn exchange(&self, cmd: u8, payload: &[u8]) -> Result<Vec<u8>, String> {
            let mut last = String::new();
            for _ in 0..3 {
                match self
                    .bus
                    .apw_exchange(apw::ADDR, &apw::frame(cmd, payload))
                    .and_then(|r| apw::parse(cmd, &r))
                {
                    Ok(p) => return Ok(p),
                    Err(e) => last = e,
                }
                ms(200);
            }
            Err(format!("PSU cmd {cmd:#04x}: {last}"))
        }

        pub fn device_type(&self) -> Result<u16, String> {
            let p = self.exchange(apw::DEVICE_TYPE, &[])?;
            Ok(u16::from_le_bytes([p[0], *p.get(1).unwrap_or(&0)]))
        }

        pub fn set_dac(&self, dac: u8) -> Result<(), String> {
            if !(apw::DAC_MIN..=apw::DAC_MAX).contains(&dac) {
                return Err(format!("DAC {dac:#04x} out of range"));
            }
            let p = self.exchange(apw::SET_DAC, &[dac, 0])?;
            if p.first() != Some(&dac) {
                return Err(format!("PSU did not echo DAC {dac:#04x}: {p:02x?}"));
            }
            Ok(())
        }

        /// Moves the output in small steps (bosminer ramps too).
        pub fn ramp_dac(&self, from: u8, to: u8) -> Result<(), String> {
            let mut d = from;
            while d != to {
                d = if d < to {
                    d.saturating_add(8).min(to)
                } else {
                    d.saturating_sub(8).max(to)
                };
                self.set_dac(d)?;
                ms(300);
            }
            Ok(())
        }

        /// Output enable is the active-low gpio907.
        pub fn enable(&self, on: bool) -> Result<(), String> {
            gpio_set(x19::PSU_ENABLE_GPIO, !on)
        }
    }

    // ---------------------------------------------------------------- PICs

    pub fn pic_addr(chain: u8) -> u8 {
        0x1F + chain
    }

    pub struct Pics {
        dev: I2cDev,
    }

    impl Pics {
        pub fn open() -> Result<Self, String> {
            Ok(Self {
                dev: I2cDev::open(0).map_err(|e| e.to_string())?,
            })
        }

        fn cmd(&mut self, addr: u8, frame: &[u8], reply: usize) -> Result<Vec<u8>, String> {
            self.dev
                .pic_transaction(addr, frame, reply)
                .map_err(|e| format!("PIC {addr:#04x}: {e}"))
        }

        /// bosminer's bring-up: reset (8 zero bytes + 0x07), jump to app (0x06),
        /// read version (0x17, expect 0x89).
        pub fn bring_up(&mut self, addr: u8) -> Result<u8, String> {
            let mut reset = vec![0u8; 8];
            reset.extend(pic::frame(0x07, &[]));
            let r = self.cmd(addr, &reset, 2)?;
            if r != [0x07, 0x01] {
                return Err(format!("PIC {addr:#04x} reset reply {r:02x?}"));
            }
            ms(500);
            let r = self.cmd(addr, &pic::frame(0x06, &[]), 2)?;
            if r != [0x06, 0x01] {
                return Err(format!("PIC {addr:#04x} jump-app reply {r:02x?}"));
            }
            ms(500);
            let r = self.cmd(addr, &pic::frame(pic::GET_VERSION, &[]), 5)?;
            if r.get(1) != Some(&pic::GET_VERSION) {
                return Err(format!("PIC {addr:#04x} version reply {r:02x?}"));
            }
            Ok(r[2])
        }

        pub fn enable_voltage(&mut self, addr: u8, on: bool) -> Result<(), String> {
            self.cmd(addr, &pic::frame(pic::ENABLE_VOLTAGE, &[u8::from(on)]), 2)
                .map(|_| ())
        }

        pub fn heartbeat(&mut self, addr: u8) -> Result<(), String> {
            let r = self.cmd(addr, &pic::frame(pic::HEARTBEAT, &[]), 6)?;
            if r.get(1) == Some(&pic::HEARTBEAT) {
                Ok(())
            } else {
                Err(format!("heartbeat reply {r:02x?}"))
            }
        }

        /// TMP75 behind the PIC (sensors 0x48..0x4b): select (0x3b), read (0x3c).
        pub fn temperature(&mut self, addr: u8, sensor: u8) -> Result<f64, String> {
            self.cmd(addr, &pic::frame(0x3b, &[sensor, 0]), 2)?;
            ms(20);
            let r = self.cmd(addr, &pic::frame(0x3c, &[sensor, 2]), 7)?;
            if r.len() < 5 || r[1] != 0x3c || r[2] != 0x01 {
                return Err(format!("temp reply {r:02x?}"));
            }
            Ok(f64::from(i16::from_be_bytes([r[3], r[4]])) / 256.0)
        }
    }

    // -------------------------------------------------------------- chains

    pub struct ChainConfig {
        pub pll0: u32,
    }

    fn bcast(c: &Chain, reg: u8, v: u32) -> Result<(), String> {
        c.send(&x19::write_reg(None, reg, v))
            .map_err(|e| e.to_string())?;
        ms(5);
        Ok(())
    }

    fn per_chip(c: &Chain, reg: u8, f: impl Fn(u8) -> u32) -> Result<(), String> {
        for i in 0..CHIPS {
            let a = i * ADDR_STEP;
            c.send(&x19::write_reg(Some(a), reg, f(a)))
                .map_err(|e| e.to_string())?;
            ms(2);
        }
        Ok(())
    }

    fn count_chips(c: &Chain) -> Result<usize, String> {
        c.drain_cmd_rx();
        c.send(&x19::read_reg(None, 0)).map_err(|e| e.to_string())?;
        let words = c.collect(Duration::from_millis(100), Duration::from_millis(2000));
        let mut addrs: Vec<u8> = x19::decode_responses(&words)
            .iter()
            .filter(|r| r.value >> 16 == 0x1398)
            .map(|r| r.chip)
            .collect();
        addrs.sort_unstable();
        addrs.dedup();
        Ok(addrs.len())
    }

    /// Changes the clock of an initialised chain (chips idle or hashing):
    /// PLL0 twice, then hash-counting and the FPGA work time for the new clock.
    pub fn set_frequency(c: &Chain, pll0: u32) -> Result<(), String> {
        let f = pll_freq_mhz(pll0);
        for _ in 0..2 {
            bcast(c, 0x08, pll0 & 0x7FFF_FFFF)?;
            ms(20);
        }
        bcast(c, 0x10, hash_counting(f))?;
        c.write_block(Block::Common, x19::common::WORK_TIME, work_time(f));
        Ok(())
    }

    /// Cold start of one chain from reset, as measured to give 100 % valid
    /// nonces. Returns the number of chips that answer at the fast baud.
    pub fn cold_init(c: &Chain, cfg: &ChainConfig) -> Result<usize, String> {
        let freq = pll_freq_mhz(cfg.pll0);
        c.write_block(Block::Common, x19::common::CTRL, 0x16);
        let g = x19::reset_gpio(c.index);
        gpio_set(g, false)?;
        ms(300);
        gpio_set(g, true)?;
        ms(1500);
        c.write_block(Block::Common, x19::common::BAUD, 0x6C);
        c.write_block(Block::Common, x19::common::CTRL, 0x1E);
        c.reset_fifos();
        if count_chips(c)? == 0 {
            return Err(format!("chain {}: no chip answers after reset", c.index));
        }
        for _ in 0..3 {
            c.send(&x19::chain_inactive()).map_err(|e| e.to_string())?;
            ms(10);
        }
        for i in 0..CHIPS {
            c.send(&x19::set_address(i * ADDR_STEP))
                .map_err(|e| e.to_string())?;
            ms(5);
        }
        per_chip(c, 0x2c, relay_value)?;
        bcast(c, 0x20, 0x0000_00FF)?;
        bcast(c, 0x1c, 0x0100_0000)?;
        bcast(c, 0x44, 0x0000_0100)?;
        bcast(c, 0x80, 0)?;
        bcast(c, 0x84, 0)?;
        bcast(c, 0x3c, 0x8000_8710)?;
        bcast(c, 0x3c, 0x8000_8050)?;
        bcast(c, 0x10, hash_counting(freq))?;
        bcast(c, 0x14, 0)?;
        bcast(c, 0xa4, 0x0000_FFFF)?;
        bcast(c, 0xa8, 0x0002_0000)?;
        bcast(c, 0x0c, 0)?;
        bcast(c, 0x70, 0)?;
        for _ in 0..2 {
            bcast(c, 0x08, cfg.pll0 & 0x7FFF_FFFF)?;
            ms(20);
        }
        ms(1000);
        bcast(c, 0x68, 0xC070_0111)?;
        bcast(c, 0x28, 0x0603_000F)?;
        ms(1000);
        per_chip(c, 0x18, misc_control)?;
        ms(1000);
        c.write_block(Block::Common, x19::common::BAUD, 0);
        c.write_block(Block::Common, x19::common::WORK_TIME, work_time(freq));
        c.reset_fifos();
        count_chips(c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_pattern_matches_board_dump() {
        assert_eq!(relay_value(0), 0x000F_0000);
        assert_eq!(relay_value(15), 0x0056_0003);
        assert_eq!(relay_value(33), 0x0050_0003);
        assert_eq!(relay_value(213), 0x0014_0003);
        assert_eq!(relay_value(18), 0x000F_0000);
        assert_eq!(
            (0..76u8)
                .filter(|i| relay_value(i * 3) != 0x000F_0000)
                .count(),
            12
        );
    }

    #[test]
    fn pll_search_hits_known_points() {
        let (_, f) = pll_for_freq(109.0);
        assert!((f - 109.0).abs() < 0.5);
        let (p, f) = pll_for_freq(675.0);
        assert!((f - 675.0).abs() < 3.0, "{f}");
        assert!((pll_freq_mhz(p) - f).abs() < 1e-6);
        for mhz in [50.0, 200.0, 333.0, 500.0, 700.0] {
            let (p, f) = pll_for_freq(mhz);
            assert!((f - mhz).abs() / mhz < 0.02, "{mhz} -> {f}");
            assert_eq!(p >> 30 & 1, 1);
        }
    }

    #[test]
    fn frequency_dependent_values() {
        assert!((pll_freq_mhz(PLL0_109MHZ) - 109.0).abs() < 0.01);
        assert!((pll_freq_mhz(0x40AD_0265) - 72.08).abs() < 0.1);
        assert!(hash_counting(109.0).abs_diff(0x9EF0) < 300);
        assert!(work_time(109.0).abs_diff(0x3FAA8) < 1000);
    }
}
