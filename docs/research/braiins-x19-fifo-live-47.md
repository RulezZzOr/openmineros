# Braiins X19 FPGA FIFO on a plain S19 (BM1398): live read on `.47`

Status: **measured on hardware, read-only**, 2026-11-22. No register was written.

## Unit

- `192.168.2.47` ("s19c"), plain Antminer S19, 3 hashboards, BM1398.
- Now runs **Braiins OS+ 26.09.1** (`2026-09-23-0-d6cb014c-26.09.1-plus`) from NAND,
  platform `zynq-bm3-am2`, kernel `4.4.0-xilinx`, DT model "Antminer X19 Miner Control Board".
  Root SSH works without a password.
- Mining in low-power mode (user: about 400 W; `power_target = 360` in `bosminer.toml`),
  ~11 TH/s, 0 hardware errors, 69 828 accepted shares at the time of reading.
- `bosminer_model.json`: model "Antminer S19", default 13.8 V / 675 MHz, 76 chips per board in an
  8×10 grid (chip chain order recorded in the file).

## Bitstream

`mtd2` (`fpga1`) and `mtd3` (`fpga2`) are identical: gzip of `system.bit`
(`system_wrapper`, Vivado 2017.4, part `7z007sclg225`, 2022-11-28).
The decompressed bitstream has the same SHA-256 (`4e2f7a0b…`) as the Braiins `system.bit.gz` from the
public `am2-s17` package (`braiins-zynq-bm3-am2.md`). **A plain S19 hashes on that public
bitstream.** The dumps are kept in the session scratchpad only, not in the repository.

## UIO map (from `/sys/class/uio`)

| Block | Address | IRQ use (`/proc/interrupts`) |
|---|---|---|
| `chainN-common` | `0x43C00000 + (N-1)*0x10000` | none |
| `chainN-cmd-rx` | `… + 0x1000` | **used**, ~5.6 M IRQs per chain in 12 days |
| `chainN-work-rx` | `… + 0x2000` | 2 (polled) |
| `chainN-work-tx` | `… + 0x3000` | 2 (polled) |
| `fan-control` | `0x42800000` | |
| `board-control` | `0x42810000` | |
| `miner-glitch-monitor` | `0x43D00000` | |

bosminer drives the chains through the **FIFO blocks**, not the PL UARTs (`ttyS*`): the
work-tx "last work ID" register advances by about `0x900` per second on every active chain.

## Register values with bosminer running

Offsets per DCENT_OS `dcentrald-hal/src/fpga_chain.rs` (GPL-3.0).

`chainN-common` (chains 1–3, identical):

| Offset | Name | Value | Meaning |
|---|---|---|---|
| `0x00` | VERSION | `0x00901002` | IP core version |
| `0x04` | BUILD_ID | `0x63848B7B` | = DCENT `BRAIINS_AM2_BITSTREAM_BUILD_ID` |
| `0x08` | CTRL | `0x0000001E` | bit 4 BM139x mode, bit 3 enable, bits 2:1 = `3` (midstate field; DCENT documents only 0/1/2) |
| `0x0C` | STAT | `0` | |
| `0x10` | BAUD | `0` | divisor 0 = fabric/16 (fabric clock of this bitstream not yet known) |
| `0x14` | WORK_TIME | `0x00055F80` | 352 128 |
| `0x18` | ERR_COUNTER | `0` | no CRC errors |

Chain 4 (no board): CTRL `0`, WORK_TIME `1` (reset value), work-tx STAT `0x14` (IRQ + TX empty).

FIFO status registers (`+0x0C`), active chains: cmd-rx `0x05` (IRQ + RX empty),
work-rx `0x01` (RX empty), work-tx `0x00` (neither empty nor full). work-tx `+0x10` threshold `0x738`.

The data FIFOs at `+0x00` of the rx blocks were **not** read: a read pops a word that belongs to
bosminer.

## What this gives us

- The register map in DCENT_OS matches this machine's bitstream (same BUILD_ID).
- Target values for our driver on a plain S19: CTRL `0x1E`, BAUD `0`, WORK_TIME `0x55F80`.
- Our `crates/board-xil` still uses the S9 single-window layout (`0x43C00000`, length `0x160`).
  That layout does not fit this bitstream and has to be replaced by a per-chain UIO backend.

## Not yet known

- Fabric clock behind BAUD `0`, so the chip-side baud rate (the chips start at 115 200 and are
  switched up during init).
- Meaning of CTRL bits 2:1 = `3`.
- The BM1398 init sequence (frequency PLL, voltage via PIC/APW). Reading it needs a capture of
  cmd-tx traffic during a bosminer restart, which stops mining for a moment.

## Chip enumeration through our own code (2026-11-22, user approved the pause)

Method: bosminer frozen with `SIGSTOP` for a few seconds (PSU, resets and fans stay as they
are), `omo-chain-probe read-reg --chain N` sends a broadcast read of register `0x00`
(`52 05 00 00`, FPGA adds preamble and CRC), then bosminer resumed with `SIGCONT`.

Result: **chains 1, 2 and 3 each answer with 76 chips**, ChipID `0x1398`, value
`0x139818xx` where `xx` is the chip address (`0x00, 0x03, 0x06, …`, step 3).

cmd-rx format, measured: the FPGA strips `AA 55`; each answer is two words,
`word0` = register value (LE word of the big-endian bytes) and
`word1` = `[chip, reg, 0x00, crc]` (bytes, LSB first).

bosminer recovered on its own after each freeze ("TX fifo on hashboard N is empty"). After
the longer work tests it raised `{ERR:H2} Zero MCR on hashboards [1]` once and restarted the
chains, then mined again after about 30 s.

## Work frames (axi_bm13xx HDL)

Ground truth is the Braiins open-source IP core `axi_bm13xx` (GPL-3.0-or-later, 2019,
`open/hw/zynq-io-am1-s9/design/src/ip_cores/axi_bm13xx/hdl/bm13xx_core.vhd` in the
HashSource/brains mirror of the old braiins-os repo). The X19 bitstream reports the same
IP version family (`0x00901002`).

- CTRL bits 2:1 = `3` means **4 midstates** (the HDL treats `10` and `11` alike). So
  CTRL `0x1E` is BM139x mode + enable + 4 midstates.
- Work frame = 36 words: `[work_id, nbits, ntime, merkle_tail]` (sent LSB first), then
  4 × 8 midstate words (sent MSB first). The chip gets `work_id & 0x7f`.
- Use work IDs that are multiples of 4: the FPGA masks the low 2 bits of the echoed ID
  (midstate index) and rebuilds the upper 9 bits from the last sent ID.
- work-rx answer: `word0` = nonce (as in the header), `word1` =
  `[crc:8 | work_id:16 | byte4:8]` (byte4 = chip "midstate_num" byte).

Measured with `omo-chain-probe work-test` (synthetic headers, ntime advanced per frame,
4 identical midstates): only the **H7..H0 numeric word order** gives valid nonces. On chain 1,
90 nonces with ≥ 32 zero bits in 1.5 s, all matching the reported job (offset 0).
The other three orders gave none.

Open: most nonces in a test do not match any job we sent, even with version bits 13–28 rolled.
Part of that is bosminer's queued work (about 51 frames stay in work-tx when it is frozen).
The rest is still unexplained (repeating IDs such as `0xaf33`–`0xaf37`). The likely cause is
chip state configured by bosminer that we do not control in a warm takeover.

### Warm-takeover work test: what the nonces are

Run on chain 2, Reversed order, 1 s (`omo-chain-probe work-test`, raw nonce list in its `all` field, analysed offline):

- 954 nonces, but only 210 distinct values. Two values (`0xb117aa48` 374×, `0x689fcf5f` 300×)
  are repeated without end. They are probably stuck cores on that board; they also showed up
  in an earlier run. A driver must drop repeated nonces.
- Of the rest, only a few distinct nonces belong to our jobs and are valid. Most carry work IDs
  that we never sent and that stay the same for the whole run (`0xaf34`, `0xfec8`, `0xff44`,
  `0xaeb8`). This bitstream does not rebuild those IDs the way the 2019 HDL does. These
  nonces are valid for none of our headers, with or without version bits 13–20 rolled.
- Work-ID stride 8 (as bosminer uses) instead of 4 gives the same picture.

Reading: the frame format is right (valid nonces match their job exactly), but in a warm
takeover most chips keep the state bosminer gave them. The next real test is a cold start
where our code sets every chip register itself.

After each work test bosminer logs `Event "crc_uart_rx" … happend ~4500–6000 times a second`
on all three boards. The common `ERR_COUNTER` (`+0x18`) is back to 0 a few seconds later.
So while our frames run, the FPGA drops thousands of chip answers per second for bad CRC5.
Under bosminer alone the counter stays at 0. This fits chips whose answer format or
configuration does not match what the frames expect, and it is one more reason to do the cold start.

## Cold start by our own code: works (2026-11-22, ~12:05 CEST)

Script `scripts/x19-cold-test.sh 1` on `.47`. The PSU stays under bosminer (frozen with
SIGSTOP after it settled at 11.96 V); everything on the board is done by `omo-chain-probe`:

1. Dump bosminer's chip registers (per chip) for reference.
2. CTRL `0x16`, reset pulse on gpio 897 (300 ms low, 1.5 s wait).
3. FPGA BAUD `0x6C` = 115 200 Bd at chip reset. CTRL `0x1E`, FIFO reset. All 76 chips answer
   at address 0. Reset defaults include PLL0 `0xC0200144`, MiscControl `0x0000BA05`,
   PLL3 `0x20700171` and FastUART `0x0600000F`.
4. Chain inactive ×3, set address `i*3` for 76 chips.
5. Registers copied from bosminer: `0x2c` per chip (UART relay on addresses 15, 33, …, 213:
   `0x00XX0003`, XX = `0x56` down to `0x14` in steps of 6; others `0x000F0000`), `0x20`, `0x1c`,
   `0x44`, `0x80`, `0x84`; core control `0x3c` ← `0x80008710`, then `0x80008050`; `0x10`
   (hash counting, bosminer uses about `0x9EF0`), `0x14` = 0, `0xa4` = `0xFFFF`, `0xa8` =
   `0x20000`, `0x0c`, `0x70`, PLL0 written twice without the lock bit.
6. Fast UART like bosminer: PLL3 `0x68` ← `0xC0700111`, FastUART `0x28` ← `0x0603000F`, then
   MiscControl `0x18` per chip (`0xF001A005`, chips 57 and 165 `0xF0018005`), then FPGA BAUD `0`.
   Writing `0x18` while the chips are still at 115 200 Bd and PLL3 is unset kills the link.
7. Work: **68-word frames with 8 midstates** (CTRL midstate field 3 means 8 on this bitstream,
   not 4 as in the 2019 HDL), work-ID stride 8, midstate words H7..H0.

Result: 4509 nonces in 4 s, **all 4509 valid** (diff1, best 41 bits). That is about
**4.8 TH/s from one board at 109 MHz** (PLL0 `0x40DA0255`), and all 8 midstate slots are used
evenly. With 36-word frames the FPGA reads two frames as one, and the IDs then come from
midstate words (`0xaf32`, `0xff45`). That was the cause of every "foreign" nonce earlier.

## Mining on all three boards with our code (2026-11-22, ~13:35 CEST)

`MODE=mine … scripts/x19-cold-test.sh "1 2 3"`, pool = the user's LAN ckpool, worker `.omo`.
The PSU is still set by bosminer (frozen at ~11.96 V). Chains 1–3 get a cold reset and init
from our code, and our thread sends the PIC heartbeat (I2C bus 0, `0x20`/`0x21`/`0x22`,
`55 AA 04 16 00 1A` every 1 s, reply `06 16 01 00 00 1D`).

| Run | Time | Hashrate | Shares | Invalid nonces |
|---|---|---|---|---|
| fixed 300 µs sleep per loop | 180 s | 9.7 TH/s | 18/18 accepted | 7 / 409 405 |
| sleep only when idle | 120 s | **14.6 TH/s** | 26/26 accepted | 8 / 374 170 |

Heartbeats: 477 ok, 0 failed. Without the heartbeat the boards lose power about 60 s after
bosminer stops (the PIC watchdog). The PIC firmware is `0x89` and GetVersion
(`55 AA 04 17 00 1B`) replies `05 17 89 00 A5`. The EEPROMs are at `0x50`–`0x52`, and the PSU
APW121215a (bosminer: version `0x71`, FW `0x0010`) is at `0x10` and not touched yet.

## Standalone: `omo-minerd` replaces bosminer (2026-11-22, ~15:50 CEST)

bosminer stopped with `/etc/init.d/bosminer stop`. `omo-minerd` did everything itself, in the
order recorded from bosminer:

1. **Fans:** board-control `+0x04` = `0x34` (C52), fan-control PWM `+0x10`/`+0x14` = 60 (percent).
2. **Hashboard PICs** (i2c-0, `0x20`–`0x22`, traced with `omo-i2c-trace`, which ptrace-starts
   bosminer and logs its i2c-dev syscalls):
   reset = 8×`00` + `55 AA 04 07 00 0B` → `07 01`; jump to app `55 AA 04 06 00 0A` → `06 01`;
   version `0x17` → `0x89`; enable voltage `55 AA 05 15 01 00 1B`; heartbeat `0x16`;
   `0x3a` reads an ADC word; temperatures through the PIC: `0x3b [sensor,0]` then
   `0x3c [sensor,2]` → `07 3C 01 HH LL …`, °C = i16(HHLL)/256, sensors `0x48`–`0x4b`.
3. **PSU APW121215a** on the bit-banged bus (AXI GPIO `0x41220000`, bit 0 = SDA, bit 1 = SCL),
   recorded with the read-only sampler `omo-chain-probe sniff` (1.9 MS/s):
   ~330 Hz bit clock (SCL low 2.1 ms with the SDA change in the middle, high 1.05 ms); every
   request byte is its own transaction `[0x10 W][0x11][byte]`, and the reply is read one byte per
   transaction. Frame `55 AA len cmd payload sum16LE`. `0x02` → type `0x0071`, `0x01` → FW
   `0x0010`, `0x83 [dac,0]` sets the output DAC (echoed). Output enable is gpio907, active low.
   Voltage is approximately `15.208 − 0.013648·DAC` (bosminer: `0x05` = 15.14 V, `0xEE` ≈ 11.96 V).
   No PSU watchdog command was seen.
4. PSU DAC `0xD8` while off, output on, PIC enable voltage on all boards, cold init of every
   chain (`x19_board::cold_init`, fixed values, no bosminer dump needed), then DAC `0xEE`.
5. Mining with PIC heartbeat and PCB temperature in one thread, max 75 °C.

Result over 3 minutes: **14.6 TH/s, 21/21 shares accepted, 9 invalid of 612 098 nonces, 423
heartbeats ok, PCB 13 °C**. On exit it switched board voltage and the PSU off.

Lesson from the 12:39 incident: never SIGSTOP bosminer. A freeze mid-I2C transaction wedged
the board-3 PIC (it answered `8a 8a …` until the miner was power-cycled).

## OpenMinerOS booted from NAND (2026-11-22, ~16:00 CEST)

`scripts/build-nand-rootfs.sh` builds a squashfs (busybox, omo-minerd, control-plane, telnetd
with login, busybox watchdog for `/dev/watchdog`, DHCP). `scripts/install-slot2.sh` runs on the
Braiins system: it copies the running kernel FIT (`/dev/ubi0_0`, d00dfeed), ubiformats `mtd8`
(firmware2), creates volumes kernel/rootfs/rootfs_data (IDs 0/1/2, matching
`ubi.block=0,1 root=/dev/ubiblock0_1`), verifies them, and sets `firmware=2 upgrade_stage=0`.
U-Boot's `auto_recovery` boots slot 2 once and sets `upgrade_stage=1`, so any later reboot
returns to Braiins in slot 1.

Result: booted in ~20 s, all three boards up 38 s after power-on, **14.9 TH/s, 23/23 shares**
in the first 70 s, PCB ~12 °C. The user confirmed that it works.

## Incident and fix: trial boot did not return (2026-11-22)

Braiins' U-Boot `auto_recovery` for `upgrade_stage=1` runs
`setenv upgrade_stage && setenv first_boot && saveenv && reset`. Braiins sets `first_boot=yes` during
its own upgrades; our installer did not. Deleting an undefined variable fails in this U-Boot, so
the `&&` chain broke and the board stayed at the U-Boot prompt on every power-up (no network). A card
without `BOOT.bin` made the fans cycle, because the board boots from SD when a card is present.

Recovery: the official Braiins SD image (`braiins-os_am2-s17_sd_2025-02-21-…-25.01-plus.img`) booted
from the card. Its mtd layout differs from the X19 one, so `fw_setenv` must not be used there. The
X19 `uboot_env` (`0x1800000`) is `mtd6` offset `0x1000000` in that layout. A corrected copy 0
(`firmware=1`, no `upgrade_stage`, flag 16) was written with `flash_erase` + `nandwrite` and verified.
Braiins then booted from NAND again.

`scripts/install-slot2.sh` now also sets `first_boot=yes`. Verified afterwards: OpenMinerOS boots
from slot 2, and a reboot through `/api/v1/x19/reboot` returns to Braiins in slot 1 by itself.

## Final version on hardware (2026-11-22, ~19:05 CEST)

Booted from slot 2 (trial): web UI and API behind Basic auth (`root` + build password), config
persisted on UBIFS (`ubi0:rootfs_data` on `/data`), default config seeded on first boot,
`PUT /api/v1/x19/config` validates and restarts mining (invalid configs get 400), auto fan control,
status JSON every 5 s. Mining: **15.0–15.1 TH/s, all shares accepted**, PCB ~8–10 °C, fans at the
30 % floor.
