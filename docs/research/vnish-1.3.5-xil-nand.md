# Interoperability notes: VNish 1.3.5 for S19 Xilinx (NAND)

These notes record what was learned by unpacking a third-party firmware update
package (`antminer-s19-xil-nand-v1.3.5.tar.gz`, build 2026-07-30) and reading
its plain-text files: shell scripts, configuration and the bundled OpenAPI
document.

## Ground rules

- That firmware is proprietary. The image is **not** part of this repository
  (`.gitignore` excludes it) and must never be committed or redistributed.
- Nothing was copied from it. No binary was unpacked, disassembled or
  decompiled. OpenMinerOS code is written from scratch against the facts below.
- Facts about the hardware platform (partition numbers, GPIO lines, device
  nodes) are recorded because a replacement firmware has to talk to the same
  control board. They are **unverified on our own hardware** until a HIL run
  confirms them — treat every number here as a hypothesis.

## Package format

| File | Purpose |
|---|---|
| `uramdisk.image.gz` | FIT image (despite the name) holding one gzip-compressed cpio ramdisk, ARM |
| `runme.sh` | Install script executed as root on the miner |
| `fw.md5` | MD5 list of the other files |
| `sign-public.pem` | Per-release signing key |
| `*.sig` | RSA/SHA-256 signatures, 512 bytes each (4096-bit key) |

Trust chain: a master public key in the running image verifies `runme.sh` and
`sign-public.pem`; the per-release key then verifies the remaining files.

## How it installs

- It replaces **only the ramdisk** in `mtd1`. Kernel, device tree, FPGA
  bitstream and boot chain stay stock. The whole OS runs from RAM.
- The stock ramdisk is kept as a backup file on the data partition, which is
  what makes "remove firmware and boot stock" possible.
- Persistent state lives on the stock UBI data partition (`mtd6`, `ubi0_0`)
  in its own subdirectory, bind-mounted to `/mnt/config` and `/mnt/data`.
- The stock configuration partition (`mtd2`) is attached read-only during
  install to carry over the serial number.

## Runtime layout

- Buildroot 2023.02.3, musl, BusyBox init with numbered `/etc/init.d/S*`
  scripts. ARM hard-float.
- Boot order: mount data → configure GPIO → hardware scan → restore missing
  config from factory defaults → network → NTP → web daemon → SSH → miner →
  process monitor.
- Three processes: a one-shot hardware scanner, a web/API daemon on port 80,
  and the mining process, which the web daemon reaches over a local
  cgminer-style API socket.
- The hardware scanner runs **before** the miner and writes a model
  description to `/var/run`, which the other two consume. Hardware detection
  is separated from mining.
- Factory defaults live read-only in the image (`/etc/factory`); a config
  file is only copied when missing. Factory reset deletes everything in the
  config directory except the serial number and MAC address.
- Logs are size-rotated per topic (status, driver, autotune, crash, miner).
- The mining process is supervised by a shell loop that polls every 30 s and
  restarts it when it is gone.

## Xilinx control board facts (to verify on hardware)

| Item | Value |
|---|---|
| PSU enable | GPIO 907, output, active high |
| Green LED | GPIO 941 |
| Red LED | GPIO 942 |
| Ramdisk partition | `/dev/mtd1` |
| Stock config partition | `mtd2` (UBI) |
| Data partition | `mtd6` (UBI, volume 0) |
| Alternate stock data partition | `mtd7` |
| Hashboard / PSU buses | `/dev/i2c-0` … `/dev/i2c-3`, FPGA registers through `/dev/mem` |

The two LED scripts in that image disagree with each other about which GPIO
is red and which is green, so LED colours need checking on a real board.

## API shape

OpenAPI 3.1, base path `/api/v1`, 46 operations. Read endpoints for status
and model information are unauthenticated; every mutation needs a bearer
token (obtained by posting the password) or an `x-api-key` header.

Ideas worth having in our own API, in our own shape:

- A **model constants** endpoint: chips per chain, chips per voltage domain,
  frequency and voltage limits, fan minimum count, temperature targets. The
  UI renders limits from it instead of hard-coding them per model.
- Miner state and per-chain state as separate enums, with time spent in the
  current state.
- Per-chain frequency, voltage, hardware errors, nominal vs. real hashrate.
- Fan state `ok` / `lost` and a configurable minimum fan count.
- Settings save returns whether a miner restart or a reboot is required.
- Settings backup / restore, and keeping settings across a firmware update.
- A time series endpoint with event annotations (start, stop, overheat,
  board disabled, firmware update) for charts.
- A "locate miner" action that blinks the LEDs.
- API keys for fleet tooling, separate from the operator password.
- Fan modes: automatic (target temperature), manual (fixed duty), immersion.

## Where OpenMinerOS deliberately differs

| Topic | Observed | OpenMinerOS |
|---|---|---|
| Update payload | Signed shell script from the bundle runs as root | Declarative manifest; a bundle cannot carry code |
| Integrity | MD5 list plus per-file signatures | SHA-256 per artifact, one signature over the manifest |
| Unlisted files | Ignored | Bundle rejected |
| Target check | Platform compared inside the script, skipped if the info file is absent | Board checked by the verifier before anything is written |
| Downgrade | Not checked | Refused unless policy allows it |
| Write strategy | Erase and rewrite the only ramdisk in place | Write the inactive slot, switch, confirm health, roll back |
| Fee pools | Reported with a pool type next to operator pools | Same transparency, default rate 0 %, opt-in |
| Pool credentials | Pool list readable without authentication | Passwords are never returned; pool users move behind authentication once REST auth lands (roadmap item 10) |
| Default SSH | Enabled, image ships an authorized key | Disabled, no baked-in keys |
| Process supervision | 30 s shell polling loop | Supervisor owns the mining lifecycle and watchdog |
| Source | Closed, including a cgminer-derived miner | GPL-3.0-only, everything public |

## Consequences for our roadmap

1. **Ramdisk-only install is the low-risk first image for Xilinx.** It leaves
   the stock boot chain untouched, so it sidesteps secure boot and keeps a
   trivial way back to stock. True A/B slots need a second ramdisk location;
   NAND layout has to be measured on hardware before that is designed.
2. **Split hardware detection from mining.** A one-shot detect step that
   writes a model description keeps the supervisor and the API independent
   of chip-level probing.
3. **Read-only factory defaults plus copy-if-missing** gives factory reset
   and config repair almost for free.
4. **Identity (serial, MAC) must survive factory reset and reinstall.**
