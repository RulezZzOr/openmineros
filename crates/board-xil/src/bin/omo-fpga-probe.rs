//! Read-only probe of the control board FPGA. Prints a JSON report.
//!
//! It never writes to the FPGA and skips registers whose read has side
//! effects, so it is safe to run next to a mining process.
//!
//! `--raw <base>` reads only the offsets in `RAW_OFFSETS` at a caller-chosen
//! physical base. Those offsets come from the stock VNish disassembly in
//! `docs/research/vnish-fpga-base.md`, and the probe does not read anything
//! else at that base.

use openmineros_board_xil::{fpga, mmio::MappedRegisters};
use std::process::ExitCode;

const FAN_SAMPLES: usize = 64;

/// Word offsets read in `--raw` mode: variant selector, the control and
/// identity words of both register variants.
const RAW_OFFSETS: [usize; 5] = [0x08, 0x80, 0x84, 0x100, 0x118];

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--raw") {
        return raw(args.get(1).map(String::as_str));
    }

    let registers = match MappedRegisters::open() {
        Ok(registers) => registers,
        Err(error) => {
            eprintln!("cannot map the FPGA register window: {error}");
            eprintln!("run as root on a Zynq control board");
            return ExitCode::FAILURE;
        }
    };

    eprintln!("mapped read-only through {}", registers.source());
    let report = fpga::probe(&registers, FAN_SAMPLES);

    match serde_json::to_string_pretty(&report) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cannot encode report: {error}");
            ExitCode::FAILURE
        }
    }
}

fn raw(base_arg: Option<&str>) -> ExitCode {
    let Some(base_text) = base_arg else {
        eprintln!("usage: omo-fpga-probe --raw <hex base, e.g. 0x40000000>");
        return ExitCode::FAILURE;
    };
    let Some(base) = parse_hex(base_text) else {
        eprintln!("invalid base address: {base_text}");
        return ExitCode::FAILURE;
    };

    let registers = match MappedRegisters::open_at(base) {
        Ok(registers) => registers,
        Err(error) => {
            eprintln!("cannot map {base:#010x} through /dev/mem: {error}");
            eprintln!("run as root on a Zynq control board");
            return ExitCode::FAILURE;
        }
    };

    let words: Vec<serde_json::Value> = RAW_OFFSETS
        .iter()
        .map(|&offset| {
            let value = registers
                .read_word(offset)
                .map_or_else(|| "unreadable".to_owned(), |word| format!("{word:#010x}"));
            serde_json::json!({ "offset": format!("{offset:#05x}"), "value": value })
        })
        .collect();

    let report = serde_json::json!({
        "base": format!("{base:#010x}"),
        "source": registers.source(),
        "words": words,
    });
    match serde_json::to_string_pretty(&report) {
        Ok(json) => {
            println!("{json}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("cannot encode report: {error}");
            ExitCode::FAILURE
        }
    }
}

fn parse_hex(text: &str) -> Option<u64> {
    let digits = text
        .strip_prefix("0x")
        .or_else(|| text.strip_prefix("0X"))
        .unwrap_or(text);
    u64::from_str_radix(digits, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_with_and_without_prefix() {
        assert_eq!(parse_hex("0x40000000"), Some(0x4000_0000));
        assert_eq!(parse_hex("40000000"), Some(0x4000_0000));
        assert_eq!(parse_hex("0X43C00000"), Some(0x43C0_0000));
    }

    #[test]
    fn rejects_non_hex_text() {
        assert_eq!(parse_hex("0xzz"), None);
        assert_eq!(parse_hex(""), None);
    }

    #[test]
    fn raw_offsets_are_aligned_and_inside_one_page() {
        for offset in RAW_OFFSETS {
            assert_eq!(offset % 4, 0, "offset {offset:#x} is not word aligned");
            assert!(offset + 4 <= 4096, "offset {offset:#x} leaves the page");
        }
    }
}
