//! FPGA register window of Zynq-based Antminer control boards.
//!
//! The register layout is the one Bitmain published under GPL-3.0-or-later
//! in the S9 `bmminer-mix` sources (`driver-btm-c5.h`). Whether the stock
//! S19 bitstream keeps this layout is **unverified**: everything decoded here
//! is a hypothesis until `omo-fpga-probe` has been run on real hardware.
//!
//! This module only reads. It contains no code path that writes a register.

use serde::Serialize;

/// Physical address of the register window on the AXI bus.
pub const FPGA_BASE_ADDRESS: u64 = 0x43C0_0000;
pub const FPGA_WINDOW_LEN: usize = 0x160;

/// Low 16 bits of the hardware version register on the S9 bitstream.
pub const S9_HARDWARE_VERSION: u32 = 0xC501;

pub const MAX_CHAINS: u8 = 16;
pub const MAX_FANS: u8 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Register {
    HardwareVersion,
    FanSpeed,
    HashOnPlug,
    BufferSpace,
    NonceNumberInFifo,
    FanControl,
    TimeOutControl,
    TicketMask,
    FpgaChipId,
    CrcErrorCount,
}

impl Register {
    /// Registers that are safe to read while a miner is running. Reading
    /// the nonce return register pops the nonce FIFO, so it is not listed.
    pub const SAFE_TO_READ: [Self; 10] = [
        Self::HardwareVersion,
        Self::FanSpeed,
        Self::HashOnPlug,
        Self::BufferSpace,
        Self::NonceNumberInFifo,
        Self::FanControl,
        Self::TimeOutControl,
        Self::TicketMask,
        Self::FpgaChipId,
        Self::CrcErrorCount,
    ];

    pub const fn offset(self) -> usize {
        match self {
            Self::HardwareVersion => 0x00,
            Self::FanSpeed => 0x04,
            Self::HashOnPlug => 0x08,
            Self::BufferSpace => 0x0c,
            Self::NonceNumberInFifo => 0x18,
            Self::FanControl => 0x84,
            Self::TimeOutControl => 0x88,
            Self::TicketMask => 0x8c,
            Self::FpgaChipId => 0xf0,
            Self::CrcErrorCount => 0xf8,
        }
    }
}

pub trait RegisterReader {
    fn read(&self, register: Register) -> u32;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct FanSample {
    pub fan: u8,
    pub rpm: u32,
}

/// The fan speed register multiplexes all fans: each read reports one fan.
pub fn decode_fan_speed(value: u32) -> FanSample {
    FanSample {
        fan: ((value >> 8) & 0x07) as u8,
        rpm: (value & 0xff) * 60 * 2,
    }
}

/// Chain slots that report a plugged-in hashboard, one bit per slot.
pub fn decode_chains_present(value: u32) -> Vec<u8> {
    (0..MAX_CHAINS)
        .filter(|chain| (value >> chain) & 1 == 1)
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RegisterValue {
    pub register: Register,
    pub offset: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProbeReport {
    pub matches_s9_layout: bool,
    pub hardware_version: String,
    pub chains_present: Vec<u8>,
    pub fans: Vec<FanSample>,
    pub registers: Vec<RegisterValue>,
    pub notes: Vec<String>,
}

pub fn probe(reader: &impl RegisterReader, fan_samples: usize) -> ProbeReport {
    let registers: Vec<RegisterValue> = Register::SAFE_TO_READ
        .into_iter()
        .map(|register| RegisterValue {
            register,
            offset: format!("{:#05x}", register.offset()),
            value: format!("{:#010x}", reader.read(register)),
        })
        .collect();

    let version = reader.read(Register::HardwareVersion);
    let matches_s9_layout = version & 0xffff == S9_HARDWARE_VERSION;
    let chains_present = decode_chains_present(reader.read(Register::HashOnPlug));

    let mut fans: Vec<FanSample> = Vec::new();
    for _ in 0..fan_samples {
        let sample = decode_fan_speed(reader.read(Register::FanSpeed));
        match fans.iter_mut().find(|fan| fan.fan == sample.fan) {
            Some(fan) => fan.rpm = fan.rpm.max(sample.rpm),
            None => fans.push(sample),
        }
    }
    fans.sort_by_key(|fan| fan.fan);

    let mut notes = Vec::new();
    if version == 0 || version == u32::MAX {
        notes.push(
            "hardware version reads as all zeros or all ones: the window is probably not mapped \
             or the bitstream is not loaded"
                .to_string(),
        );
    } else if !matches_s9_layout {
        notes.push(
            "hardware version differs from the S9 bitstream: decoded chains and fans below are \
             guesses, send this report before trusting them"
                .to_string(),
        );
    }
    if chains_present.len() > 4 {
        notes.push(
            "more than four chains reported present: the hash-on-plug register is probably at a \
             different offset on this bitstream"
                .to_string(),
        );
    }

    ProbeReport {
        matches_s9_layout,
        hardware_version: format!("{version:#010x}"),
        chains_present,
        fans,
        registers,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct FakeFpga {
        version: u32,
        hash_on_plug: u32,
        fan_reads: Cell<u32>,
    }

    impl RegisterReader for FakeFpga {
        fn read(&self, register: Register) -> u32 {
            match register {
                Register::HardwareVersion => self.version,
                Register::HashOnPlug => self.hash_on_plug,
                Register::FanSpeed => {
                    let read = self.fan_reads.get();
                    self.fan_reads.set(read + 1);
                    let fan = read % 4;
                    (fan << 8) | (48 + fan)
                }
                _ => 0,
            }
        }
    }

    fn fake(version: u32, hash_on_plug: u32) -> FakeFpga {
        FakeFpga {
            version,
            hash_on_plug,
            fan_reads: Cell::new(0),
        }
    }

    #[test]
    fn decodes_fan_id_and_rpm() {
        assert_eq!(
            decode_fan_speed(0x0000_0230),
            FanSample { fan: 2, rpm: 5760 }
        );
    }

    #[test]
    fn decodes_s19_style_three_chain_layout() {
        assert_eq!(decode_chains_present(0b0111), vec![0, 1, 2]);
        assert_eq!(decode_chains_present(0b0111_0000), vec![4, 5, 6]);
        assert!(decode_chains_present(0).is_empty());
    }

    #[test]
    fn recognises_the_s9_bitstream() {
        let report = probe(&fake(0x0001_C501, 0b0111), 16);

        assert!(report.matches_s9_layout);
        assert!(report.notes.is_empty());
        assert_eq!(report.chains_present, vec![0, 1, 2]);
        assert_eq!(report.fans.len(), 4);
        assert_eq!(report.fans[3], FanSample { fan: 3, rpm: 6120 });
    }

    #[test]
    fn flags_an_unknown_bitstream_instead_of_trusting_it() {
        let report = probe(&fake(0x0000_B013, 0xffff), 4);

        assert!(!report.matches_s9_layout);
        assert_eq!(report.notes.len(), 2);
    }

    #[test]
    fn flags_an_unmapped_window() {
        let report = probe(&fake(u32::MAX, 0), 1);

        assert!(report.notes[0].contains("not mapped"));
    }

    #[test]
    fn never_reads_outside_the_window_or_the_nonce_fifo() {
        for register in Register::SAFE_TO_READ {
            assert!(register.offset() + 4 <= FPGA_WINDOW_LEN);
            assert_eq!(register.offset() % 4, 0);
            assert_ne!(register.offset(), 0x10, "reading 0x10 pops the nonce FIFO");
        }
    }
}
