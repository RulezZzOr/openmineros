# Hashboard driver: what is public and what is missing

Status on 2026-09-27. Target: Antminer S19-class on the Xilinx (Zynq) control
board.

## Correction, later on 2026-09-27

The first version of this note said that no open driver exists for S19
hashboards on the Xilinx control board and none for BM1398. That was wrong.
[DCentralTech/DCENT_OS](https://github.com/DCentralTech/DCENT_OS) published
its source under GPL-3.0 (public mirror synced 2026-08-31). Sections below
that say otherwise are superseded by this one.

What its source and `docs/PLATFORMS.md` state:

| Machine | Chip | Control board | Their status |
|---|---|---|---|
| S19j Pro | BM1362 | Zynq | mining achieved, accepted pool shares |
| S19 Pro | BM1398 | Zynq | nonces produced on a lab unit, no accepted share proven |
| **S19** | BM1398 | Zynq | management only, no mining runtime |

How they reach the chain on Zynq: not through the stock bitstream. They boot
from SD with a different, documented FPGA bitstream (the Braiins one) plus a
matching kernel and device tree, and talk to the FPGA through UIO devices.
Their docs call the stock Bitmain fabric "a different register/DMA world".

Relevant files in their tree (`DCENT_OS_Antminer/dcentrald/`):

- `dcentrald-hal/src/platform/zynq.rs`, `fpga_chain.rs` – Zynq transport
- `dcentrald-asic/src/drivers/bm1398.rs` – experimental BM1398 driver
- `dcentrald_s19.toml` – conservative first-test settings for a plain S19
- `docs/INSTALL/S19J_PRO_XILINX.md` – the SD install route

Closed firmware, for the record: Braiins publishes source for the S9 only
(BM1387), S19 support lives in the closed Braiins OS+. VNish and NiceHash
publish no source.

## The three layers of a driver

| Layer | Job | Public, reusable source |
|---|---|---|
| Chip protocol | BM13xx frames, CRC, chip enumeration, PLL, job and nonce format | Yes |
| Board peripherals | PSU (APW12), voltage controller (PIC), temperature sensors, EEPROM, fans | Yes, for the Amlogic control board |
| Transport | How bytes reach the chain on this control board | **No, not for Xilinx** |

## Public sources

| Source | Licence | What it gives us |
|---|---|---|
| [256foundation/mujina](https://github.com/256foundation/mujina) | GPL-3.0-or-later | Rust BM13xx protocol, chain sequencer, voltage-frequency ramp, BM1362 configuration |
| [Schnitzel/mujina](https://github.com/Schnitzel/mujina), branches `amlogic-s19jpro-pic-support`, `amlogic-s19kpro-support` | GPL-3.0-or-later | S19j Pro (126 × BM1362) and S19k Pro on the **Amlogic** control board: APW12, PIC, TMP75, fans, EEPROM |
| [bitmaintech/bmminer-mix](https://github.com/bitmaintech/bmminer-mix) | GPL-3.0-or-later | Bitmain's own S9 miner source with the Zynq FPGA register map |
| `braiins/zynq-io` (not checked here) | open | Braiins' replacement S9 bitstream, not the stock one |

GPL-3.0-or-later code can be used in this GPL-3.0-only project with
attribution.

Mujina's own compatibility page lists S19j/S19k Pro as "in a fork", not
production-ready, with an unsettled install path.

## The reference machine: plain S19

The project's test machine is a plain Antminer S19 on the Xilinx control
board. Its hashboards carry BM1398 chips (76 per board, three boards).

None of the sources above contain a BM1398 driver. The closest open driver is
for the BM1397 (Antminer S17) in the Bitaxe ESP-Miner firmware. BM1398 belongs
to the same family, but how far the register set and the initialisation
sequence match is unverified. So on this machine two of the three layers are
open questions: the FPGA transport and the chip configuration.

## Why Xilinx is the hard one

On the Amlogic control board the hashboard UARTs are ordinary Linux serial
ports, so the open chip protocol can talk to the chain directly.

On the Zynq control board the CPU never touches the chain. Work and commands
go through FIFOs inside the FPGA, which drives the chain UARTs itself. The
bitstream is signed and stays stock. No public document describes the
register interface of the **S19** stock bitstream. The only published map is
for the S9.

Nobody has published an open driver for S19 hashboards on the Xilinx control
board.

## What is in this repository

`crates/board-xil/src/fpga.rs` holds the S9 register map as a hypothesis, and
`omo-fpga-probe` tests that hypothesis on real hardware:

- maps the register window read-only (`PROT_READ`), so it cannot write,
- reads only registers without read side effects (it skips the nonce FIFO),
- prints a JSON report: hardware version, chains present, fan speeds.

```sh
scp dist/omo-fpga-probe-armv7 root@<miner>:/tmp/
ssh root@<miner> /tmp/omo-fpga-probe-armv7
```

It can run on a miner that is mining on its current firmware.

## Path to a hashing Xilinx build

1. Run the probe on a real Xilinx S19. If the hardware version and the
   chain and fan readings make sense, the S9 map is a usable base.
2. Read the chip identification of the BM1398 chain and compare its
   behaviour with the open BM1397 driver.
3. Bring up peripherals read-only first: EEPROM, temperature sensors, PSU
   telemetry.
4. Chain enumeration at the lowest voltage and frequency, one hashboard,
   fans at full speed, with a hardware watchdog on the PSU.
5. Only then work submission, nonce validation and the pool client.

Every step from 3 on needs a person at the machine. Wrong voltage or a
missing thermal cut-off destroys hashboards, so none of it is shipped
untested.

## The shorter path

An Amlogic control board swaps into the same chassis and has working open
support for S19j Pro hashboards today. If hashing on open firmware matters
more than the Xilinx board itself, that is the fastest route.
