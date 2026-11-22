//! Chain FIFO interface of the Braiins `zynq-bm3-am2` ("X19") bitstream.
//!
//! Register offsets and values were read on a plain S19 running Braiins OS
//! (see `docs/research/braiins-x19-fifo-live-47.md`). Each chain has four
//! 4 KiB UIO blocks: `chainN-common`, `chainN-cmd-rx`, `chainN-work-rx` and
//! `chainN-work-tx`.

use serde::Serialize;

/// BUILD_ID of the public Braiins am2 bitstream (2022-11-28).
pub const BUILD_ID_AM2: u32 = 0x6384_8B7B;

pub mod common {
    pub const VERSION: usize = 0x00;
    pub const BUILD_ID: usize = 0x04;
    /// bit 4 BM139x mode, bit 3 enable, bits 2:1 midstate count, bit 0 error clear.
    pub const CTRL: usize = 0x08;
    pub const STAT: usize = 0x0C;
    pub const BAUD: usize = 0x10;
    pub const WORK_TIME: usize = 0x14;
    pub const ERR_COUNTER: usize = 0x18;
}

pub mod fifo {
    /// Data FIFO. Reading it pops a word (rx blocks).
    pub const DATA_RX: usize = 0x00;
    /// Data FIFO input (tx side of the cmd and work-tx blocks).
    pub const DATA_TX: usize = 0x04;
    pub const CTRL: usize = 0x08;
    pub const STAT: usize = 0x0C;
    pub const WORK_TX_THRESHOLD: usize = 0x10;
    pub const WORK_TX_LAST_ID: usize = 0x14;

    pub const STAT_RX_EMPTY: u32 = 1 << 0;
    pub const STAT_RX_FULL: u32 = 1 << 1;
    pub const STAT_TX_EMPTY: u32 = 1 << 2;
    pub const STAT_TX_FULL: u32 = 1 << 3;
    /// Set while the FIFO level is below the threshold register.
    pub const STAT_BELOW_THRESHOLD: u32 = 1 << 4;
    /// work-tx FIFO depth in words (axi_bm13xx `fifo_block_thr`).
    pub const WORK_TX_DEPTH: u32 = 2048;
    /// Threshold bosminer uses: depth minus the biggest frame (200 words).
    pub const WORK_TX_THRESHOLD_DEFAULT: u32 = WORK_TX_DEPTH - 200;
}

/// Values bosminer leaves on a mining plain S19 (BM1398).
pub mod s19_running {
    pub const CTRL: u32 = 0x1E;
    pub const BAUD: u32 = 0;
    pub const WORK_TIME: u32 = 0x0005_5F80;
}

/// BM139x command header (BM1397 family protocol).
pub mod cmd {
    pub const TYPE_CMD: u8 = 0x40;
    pub const ALL: u8 = 0x10;
    pub const SET_ADDRESS: u8 = 0x00;
    pub const WRITE_REG: u8 = 0x01;
    pub const READ_REG: u8 = 0x02;
    pub const CHAIN_INACTIVE: u8 = 0x03;
}

/// Command bytes without preamble and CRC (the FPGA adds both).
/// The length byte counts header, length, payload and the CRC byte.
pub fn read_reg(chip: Option<u8>, reg: u8) -> Vec<u8> {
    let all = if chip.is_none() { cmd::ALL } else { 0 };
    vec![
        cmd::TYPE_CMD | all | cmd::READ_REG,
        5,
        chip.unwrap_or(0),
        reg,
    ]
}

pub fn write_reg(chip: Option<u8>, reg: u8, value: u32) -> Vec<u8> {
    let all = if chip.is_none() { cmd::ALL } else { 0 };
    let mut out = vec![
        cmd::TYPE_CMD | all | cmd::WRITE_REG,
        9,
        chip.unwrap_or(0),
        reg,
    ];
    out.extend_from_slice(&value.to_be_bytes());
    out
}

pub fn set_address(chip: u8) -> Vec<u8> {
    vec![cmd::TYPE_CMD | cmd::SET_ADDRESS, 5, chip, 0]
}

pub fn chain_inactive() -> Vec<u8> {
    vec![cmd::TYPE_CMD | cmd::ALL | cmd::CHAIN_INACTIVE, 5, 0, 0]
}

/// Packs command bytes LSB-first into FIFO words, zero-padding the tail.
pub fn pack_words(bytes: &[u8]) -> Vec<u32> {
    bytes
        .chunks(4)
        .map(|c| {
            let mut b = [0u8; 4];
            b[..c.len()].copy_from_slice(c);
            u32::from_le_bytes(b)
        })
        .collect()
}

/// One register-read response from a chip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegResponse {
    pub value: u32,
    pub chip: u8,
    pub reg: u8,
    /// Byte 6 of the FIFO pair (0 in every answer seen so far).
    pub flags: u8,
    /// Byte 7 of the FIFO pair, the chip's CRC/status byte.
    pub crc: u8,
}

/// Decodes cmd-rx words. The FPGA strips the `AA 55` preamble and delivers
/// each answer as two words: the register value (big-endian bytes) and
/// `[chip, reg, flags, crc]`. Measured on a plain S19 (BM1398), 2026-11-22.
pub fn decode_responses(words: &[u32]) -> Vec<RegResponse> {
    words
        .chunks_exact(2)
        .map(|pair| {
            let v = pair[0].to_le_bytes();
            let t = pair[1].to_le_bytes();
            RegResponse {
                value: u32::from_be_bytes(v),
                chip: t[0],
                reg: t[1],
                flags: t[2],
                crc: t[3],
            }
        })
        .collect()
}

#[derive(Debug, Clone, Serialize)]
pub struct ChainSnapshot {
    pub chain: u8,
    pub version: String,
    pub build_id: String,
    pub ctrl: String,
    pub baud: String,
    pub work_time: String,
    pub err_counter: u32,
    pub cmd_stat: String,
    pub work_rx_stat: String,
    pub work_tx_stat: String,
    pub work_tx_last_id: String,
    pub known_bitstream: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Common,
    Cmd,
    WorkRx,
    WorkTx,
}

impl std::str::FromStr for Block {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "common" => Ok(Self::Common),
            "cmd" => Ok(Self::Cmd),
            "work-rx" => Ok(Self::WorkRx),
            "work-tx" => Ok(Self::WorkTx),
            _ => Err(format!("unknown block {s}")),
        }
    }
}

/// Sysfs GPIO that holds chain `index` (1-based) in reset when 0.
/// Measured on `.47`: gpio 897..=899 are chains 1..=3 (`gpio@41210000`).
pub fn reset_gpio(index: u8) -> u32 {
    896 + u32::from(index)
}

/// Active-low PSU enable on the X19 control board (gpio 907, MIO 1):
/// 0 while mining, 1 after bosminer logged "PSU: Disable".
pub const PSU_ENABLE_GPIO: u32 = 907;

/// BM1398 PLL0 value: `25 MHz * fbdiv / (refdiv * postdiv1 * postdiv2)`,
/// bit 30 = PLL enable (layout per DCENT_OS `bm1398.rs`, reset default
/// `0xC0600161` = 400 MHz).
pub fn pll0(fbdiv: u32, refdiv: u32, postdiv1: u32, postdiv2: u32) -> u32 {
    (1 << 30)
        | ((fbdiv & 0xFFF) << 16)
        | ((refdiv & 0x3F) << 8)
        | ((postdiv1 & 7) << 4)
        | (postdiv2 & 7)
}

#[cfg(unix)]
pub use hw::Chain;

#[cfg(unix)]
mod hw {
    use super::*;
    use crate::uio::UioDevice;
    use std::{
        io, thread,
        time::{Duration, Instant},
    };

    pub struct Chain {
        pub index: u8,
        common: UioDevice,
        cmd: UioDevice,
        work_rx: UioDevice,
        work_tx: UioDevice,
    }

    impl Chain {
        /// Opens chain `index` (1-based, as in the UIO names).
        pub fn open(index: u8) -> io::Result<Self> {
            let open = |block: &str| UioDevice::open_by_name(&format!("chain{index}-{block}"));
            Ok(Self {
                index,
                common: open("common")?,
                cmd: open("cmd-rx")?,
                work_rx: open("work-rx")?,
                work_tx: open("work-tx")?,
            })
        }

        /// Reads identity, control and status registers. Never touches a data FIFO.
        pub fn snapshot(&self) -> ChainSnapshot {
            let hex = |v: u32| format!("{v:#010x}");
            let build = self.common.read(common::BUILD_ID);
            ChainSnapshot {
                chain: self.index,
                version: hex(self.common.read(common::VERSION)),
                build_id: hex(build),
                ctrl: hex(self.common.read(common::CTRL)),
                baud: hex(self.common.read(common::BAUD)),
                work_time: hex(self.common.read(common::WORK_TIME)),
                err_counter: self.common.read(common::ERR_COUNTER),
                cmd_stat: hex(self.cmd.read(fifo::STAT)),
                work_rx_stat: hex(self.work_rx.read(fifo::STAT)),
                work_tx_stat: hex(self.work_tx.read(fifo::STAT)),
                work_tx_last_id: hex(self.work_tx.read(fifo::WORK_TX_LAST_ID)),
                known_bitstream: build == BUILD_ID_AM2,
            }
        }

        /// Raw register write into one of the four blocks.
        pub fn write_block(&self, block: Block, offset: usize, value: u32) {
            self.block(block).write(offset, value);
        }

        pub fn read_block(&self, block: Block, offset: usize) -> u32 {
            self.block(block).read(offset)
        }

        fn block(&self, block: Block) -> &UioDevice {
            match block {
                Block::Common => &self.common,
                Block::Cmd => &self.cmd,
                Block::WorkRx => &self.work_rx,
                Block::WorkTx => &self.work_tx,
            }
        }

        /// Resets the cmd, work-rx and work-tx FIFOs (drops queued data).
        pub fn reset_fifos(&self) {
            self.cmd.write(fifo::CTRL, 0x3);
            self.work_rx.write(fifo::CTRL, 0x1);
            self.work_tx.write(fifo::CTRL, 0x2);
            thread::sleep(Duration::from_millis(2));
            self.cmd.write(fifo::CTRL, 0);
            self.work_rx.write(fifo::CTRL, 0);
            self.work_tx.write(fifo::CTRL, 0);
        }

        pub fn build_id(&self) -> u32 {
            self.common.read(common::BUILD_ID)
        }

        /// Pops every word currently in the cmd-rx FIFO (bounded).
        pub fn drain_cmd_rx(&self) -> Vec<u32> {
            let mut out = Vec::new();
            while out.len() < 4096 && self.cmd.read(fifo::STAT) & fifo::STAT_RX_EMPTY == 0 {
                out.push(self.cmd.read(fifo::DATA_RX));
            }
            out
        }

        /// Writes one command to the cmd-tx FIFO, waiting while it is full.
        pub fn send(&self, bytes: &[u8]) -> io::Result<()> {
            for word in pack_words(bytes) {
                let deadline = Instant::now() + Duration::from_millis(100);
                while self.cmd.read(fifo::STAT) & fifo::STAT_TX_FULL != 0 {
                    if Instant::now() > deadline {
                        return Err(io::Error::new(io::ErrorKind::TimedOut, "cmd-tx FIFO full"));
                    }
                    thread::sleep(Duration::from_micros(200));
                }
                self.cmd.write(fifo::DATA_TX, word);
            }
            Ok(())
        }

        /// Collects cmd-rx words until `quiet` passes without new data or
        /// `max` elapses.
        pub fn collect(&self, quiet: Duration, max: Duration) -> Vec<u32> {
            let start = Instant::now();
            let mut last = Instant::now();
            let mut out = Vec::new();
            while start.elapsed() < max {
                let got = self.drain_cmd_rx();
                if got.is_empty() {
                    if last.elapsed() >= quiet {
                        break;
                    }
                    thread::sleep(Duration::from_millis(1));
                } else {
                    out.extend(got);
                    last = Instant::now();
                }
            }
            out
        }
        pub fn work_tx_full(&self) -> bool {
            self.work_tx.read(fifo::STAT) & fifo::STAT_TX_FULL != 0
        }

        /// Writes one work frame to work-tx. Returns false (nothing written)
        /// when the FIFO reports full.
        /// Sets the work-tx threshold so that "below threshold" guarantees
        /// room for one more frame of up to 200 words.
        pub fn init_work_tx_threshold(&self) {
            self.work_tx
                .write(fifo::WORK_TX_THRESHOLD, fifo::WORK_TX_THRESHOLD_DEFAULT);
        }

        /// Writes a whole frame only when the FIFO has room for it. Writing
        /// into a FIFO that fills up mid-frame drops words and shifts every
        /// later frame (measured: bogus work IDs equal to midstate words).
        pub fn push_work(&self, words: &[u32]) -> bool {
            assert!(words.len() <= 200, "frame longer than the threshold margin");
            let stat = self.work_tx.read(fifo::STAT);
            if stat & fifo::STAT_TX_FULL != 0 || stat & fifo::STAT_BELOW_THRESHOLD == 0 {
                return false;
            }
            for (i, w) in words.iter().enumerate() {
                self.work_tx.write(fifo::DATA_TX, *w);
                if i % 4 == 3 {
                    // Read back status to flush posted AXI writes.
                    let _ = self.work_tx.read(fifo::STAT);
                }
            }
            true
        }

        /// Pops one nonce (two words) from work-rx.
        pub fn pop_nonce(&self) -> Option<(u32, u32)> {
            if self.work_rx.read(fifo::STAT) & fifo::STAT_RX_EMPTY != 0 {
                return None;
            }
            let w0 = self.work_rx.read(fifo::DATA_RX);
            let w1 = self.work_rx.read(fifo::DATA_RX);
            Some((w0, w1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pll0_reset_default_round_trips() {
        assert_eq!(pll0(96, 1, 6, 1) | (1 << 31), 0xC060_0161);
        assert_eq!(pll0(96, 1, 6, 4), 0x4060_0164);
    }

    #[test]
    fn broadcast_read_packs_like_the_fpga_expects() {
        assert_eq!(pack_words(&read_reg(None, 0)), vec![0x0000_0552]);
        assert_eq!(pack_words(&read_reg(Some(8), 0x18)), vec![0x1808_0542]);
        assert_eq!(pack_words(&chain_inactive()), vec![0x0000_0553]);
        assert_eq!(pack_words(&set_address(4)), vec![0x0004_0540]);
    }

    #[test]
    fn write_reg_is_two_words_with_big_endian_value() {
        let words = pack_words(&write_reg(None, 0x18, 0x1234_5678));
        assert_eq!(words, vec![0x1800_0951, 0x7856_3412]);
    }

    #[test]
    fn decodes_fifo_pairs_as_measured_on_bm1398() {
        // First two answers to a broadcast ChipID read on `.47`, chain 1.
        let words = [0x0018_9813, 0x0400_0000, 0x0318_9813, 0x0b00_0003];
        let got = decode_responses(&words);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].value, 0x1398_1800);
        assert_eq!((got[0].chip, got[0].reg, got[0].crc), (0, 0, 0x04));
        assert_eq!(got[1].value, 0x1398_1803);
        assert_eq!((got[1].chip, got[1].crc), (3, 0x0b));
    }
}
