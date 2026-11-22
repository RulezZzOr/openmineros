# Miner 192.168.2.47 (s19c, AnthillOS): log facts

Read-only, through the web UI logs panel in Chrome on 2026-10-10. Nothing was
written to the miner. Facts are as logged by the firmware; they are not yet
verified against registers.

## Firmware and system

- Web title `s19c`. Firmware init line: `Antminer S19 Xilinx (1.2.7)`.
  The other unit (`.41`) reported `(1.3.5)`.
- Kernel: `Linux version 4.6.0-xilinx-g03c746f7`, built 2020-09-21.
- `CPU1: failed to boot: -1` also appears here, so it is a normal state of this
  kernel and not a cause of the `.41` outage.
- Root filesystem is JFFS2 on NAND. `.41` used UBI on `mtd6`.
- The logs carry no real clock before the first NTP sync: entries start at
  `1970/01/01`.

## FPGA and chains

- `Detected 256 Mb of RAM`.
- `Set HW version to 0x4000b031`, then `Starting FPGA queue`.
  The value `0x4000b031` is nonzero with bit 31 clear. The stock selector
  reads word 2 (`0x40000008`) and picks variant 1 when that holds. If the HW
  version register is that word, this unit is variant 1, and its control
  register is at `0x40000100`. **Hypothesis, not checked.**
- `chain#1`, `chain#2`, `chain#3` detected. `chain#3 - 76 chips detected`.
- Frequency ramp: preheat, then `raising freq to 368/371/374 MHz`, then
  `Raising freq from 368 to 380 Mhz (chip by chip)`.
- `Restarting (1 of 30) - Chain break detected` appears 4 times in the status
  log (most recent 2026-10-08 15:21). The firmware retries up to 30 times.

## Relevance

- 76 chips per board matches `.41`, so the chain geometry is the same family.
- The chain-break restart logic is a documented recovery path in this firmware.
  It is a possible but unproven cause for the `.41` drop.
- The HW version value is the most useful number here. Reading
  `devmem 0x40000008 32` on a unit that runs this firmware would confirm or
  reject the hypothesis.
