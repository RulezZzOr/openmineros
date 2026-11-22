//! Minimal Linux i2c-dev access and the hashboard voltage-controller (PIC)
//! frames used on S19 boards.
//!
//! PIC frame: `55 AA <len> <cmd> <payload…> <sum16 BE>`, where `len` counts
//! len + cmd + payload + 2 checksum bytes and the checksum is the 16-bit sum
//! of len, cmd and payload. GetVersion (`0x17`) and Heartbeat (`0x16`) frames
//! match DCENT_OS `bb_pic_application.rs` / `bb_pic_heartbeat.rs`; the
//! command numbers match Braiins' GPL S9 `power.rs`.

pub mod pic {
    pub const ENABLE_VOLTAGE: u8 = 0x15;
    pub const HEARTBEAT: u8 = 0x16;
    pub const GET_VERSION: u8 = 0x17;

    pub fn frame(cmd: u8, payload: &[u8]) -> Vec<u8> {
        let len = (payload.len() + 4) as u8;
        let sum: u16 = payload
            .iter()
            .fold(u16::from(len) + u16::from(cmd), |a, &b| {
                a.wrapping_add(u16::from(b))
            });
        let mut out = vec![0x55, 0xAA, len, cmd];
        out.extend_from_slice(payload);
        out.extend_from_slice(&sum.to_be_bytes());
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn frames_match_known_vectors() {
            assert_eq!(frame(HEARTBEAT, &[]), [0x55, 0xAA, 0x04, 0x16, 0x00, 0x1A]);
            assert_eq!(
                frame(GET_VERSION, &[]),
                [0x55, 0xAA, 0x04, 0x17, 0x00, 0x1B]
            );
        }
    }
}

/// APW121215a PSU protocol, recovered by passively sampling bosminer on the
/// bit-banged PSU bus (gpio895/896, see `bitbang.rs`) on `.47`, 2026-11-22.
///
/// Frame: `55 AA <len> <cmd> <payload…> <sum16 LE>`; `len` counts len, cmd,
/// payload and the two checksum bytes; the checksum is the 16-bit sum of
/// len, cmd and payload. Every byte goes out as its own I2C write
/// `[0x10 W] [0x11] [byte]`; the reply is read one byte per transaction.
pub mod apw {
    pub const ADDR: u8 = 0x10;
    pub const WRITE_REG: u8 = 0x11;
    pub const FW_VERSION: u8 = 0x01;
    pub const DEVICE_TYPE: u8 = 0x02;
    pub const GET_DAC: u8 = 0x03;
    pub const READ_BLOCK: u8 = 0x06;
    pub const SET_DAC: u8 = 0x83;
    /// DAC limits bosminer used: 0x05 = 15.14 V (init), 0xEE ≈ 12.0 V.
    pub const DAC_MIN: u8 = 0x05;
    pub const DAC_MAX: u8 = 0xF5;

    pub fn frame(cmd: u8, payload: &[u8]) -> Vec<u8> {
        let len = (payload.len() + 4) as u8;
        let sum = payload
            .iter()
            .fold(u16::from(len) + u16::from(cmd), |a, &b| {
                a.wrapping_add(u16::from(b))
            });
        let mut out = vec![0x55, 0xAA, len, cmd];
        out.extend_from_slice(payload);
        out.extend_from_slice(&sum.to_le_bytes());
        out
    }

    /// Validates a reply frame and returns its payload.
    pub fn parse(cmd: u8, reply: &[u8]) -> Result<Vec<u8>, String> {
        if reply.len() < 6 || reply[0] != 0x55 || reply[1] != 0xAA {
            return Err(format!("bad preamble {reply:02x?}"));
        }
        let len = usize::from(reply[2]);
        if reply.len() != len + 2 || len < 4 {
            return Err(format!("bad length {reply:02x?}"));
        }
        if reply[3] != cmd {
            return Err(format!(
                "reply for cmd {:#04x}, wanted {cmd:#04x}",
                reply[3]
            ));
        }
        let body = &reply[2..len];
        let sum = body.iter().fold(0u16, |a, &b| a.wrapping_add(u16::from(b)));
        if sum.to_le_bytes() != [reply[len], reply[len + 1]] {
            return Err(format!("bad checksum {reply:02x?}"));
        }
        Ok(reply[4..len].to_vec())
    }

    /// Approximate output voltage for a DAC code, fitted to bosminer's ramp
    /// (0x05 = 15.14 V, 0xEE ≈ 11.96–12.1 V). Used for logging and limits only.
    pub fn volts_for_dac(dac: u8) -> f64 {
        15.208 - 0.013648 * f64::from(dac)
    }

    pub fn dac_for_volts(v: f64) -> u8 {
        ((15.208 - v) / 0.013648)
            .round()
            .clamp(f64::from(DAC_MIN), f64::from(DAC_MAX)) as u8
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn frames_match_bosminer_capture() {
            assert_eq!(
                frame(DEVICE_TYPE, &[]),
                [0x55, 0xAA, 0x04, 0x02, 0x06, 0x00]
            );
            assert_eq!(
                frame(READ_BLOCK, &[0x40, 0x20]),
                [0x55, 0xAA, 0x06, 0x06, 0x40, 0x20, 0x6C, 0x00]
            );
            assert_eq!(
                frame(SET_DAC, &[0xD8, 0x00]),
                [0x55, 0xAA, 0x06, 0x83, 0xD8, 0x00, 0x61, 0x01]
            );
        }

        #[test]
        fn parses_captured_replies() {
            assert_eq!(
                parse(
                    DEVICE_TYPE,
                    &[0x55, 0xAA, 0x06, 0x02, 0x71, 0x00, 0x79, 0x00]
                )
                .unwrap(),
                [0x71, 0x00]
            );
            assert_eq!(
                parse(
                    FW_VERSION,
                    &[0x55, 0xAA, 0x06, 0x01, 0x10, 0x00, 0x17, 0x00]
                )
                .unwrap(),
                [0x10, 0x00]
            );
            assert!(parse(GET_DAC, &[0x55, 0xAA, 0x06, 0x03, 0xFF, 0x00, 0x08, 0x02]).is_err());
            assert!((volts_for_dac(dac_for_volts(12.0)) - 12.0).abs() < 0.01);
        }
    }
}

#[cfg(unix)]
pub use dev::I2cDev;

#[cfg(unix)]
mod dev {
    use std::{
        fs::{File, OpenOptions},
        io::{self, Read, Write},
        os::fd::AsRawFd,
        thread,
        time::Duration,
    };

    const I2C_SLAVE: libc::c_ulong = 0x0703;

    pub struct I2cDev {
        file: File,
    }

    impl I2cDev {
        pub fn open(bus: u8) -> io::Result<Self> {
            Ok(Self {
                file: OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(format!("/dev/i2c-{bus}"))?,
            })
        }

        fn select(&self, addr: u8) -> io::Result<()> {
            // SAFETY: I2C_SLAVE takes the 7-bit address as an integer argument.
            let rc = unsafe {
                libc::ioctl(
                    self.file.as_raw_fd(),
                    I2C_SLAVE as _,
                    libc::c_ulong::from(addr),
                )
            };
            if rc < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        }

        /// Writes `frame` one byte per I2C transaction (as the vendor code
        /// does), waits, then reads `reply_len` bytes one at a time.
        pub fn pic_transaction(
            &mut self,
            addr: u8,
            frame: &[u8],
            reply_len: usize,
        ) -> io::Result<Vec<u8>> {
            self.select(addr)?;
            for b in frame {
                self.file.write_all(&[*b])?;
            }
            thread::sleep(Duration::from_millis(30));
            let mut out = Vec::with_capacity(reply_len);
            for _ in 0..reply_len {
                let mut b = [0u8; 1];
                self.file.read_exact(&mut b)?;
                out.push(b[0]);
                thread::sleep(Duration::from_millis(2));
            }
            Ok(out)
        }
    }
}
