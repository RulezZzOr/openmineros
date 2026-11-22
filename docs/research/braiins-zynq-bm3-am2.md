# Braiins OS packages for the Antminer X19 Zynq control board

Status: **read from the public packages only. Nothing here has been tested on our hardware.**
Examined 2026-10-15.

## Sources

Public downloads from the Braiins feed index (`https://downloads.braiins-os.com/braiins-os/index.json`).
Each download was checked against the SHA-256 listed in that index. The files stay in the session
scratchpad and are not committed. Only facts are written down here: addresses, file names, hashes and
what the boot scripts do. No code is copied.

| Package | Asset key | SHA-256 |
|---|---|---|
| `braiins-os_zynq-bm3-am2_ssh_2026-09-23-0-d6cb014c-26.09.1-plus.tar.gz` | `transitional_zynq_bm3_am2` | `63fbaac7…` (index) |
| `am2-s17/sd-firmware_2023-11-20-0-b982cd98-23.10.1-plus_arm_cortex-a9_neon.tar` | `sysupgrade_sd_am2_s17` | `5d03dd912448267152e64681757c8f88282dea54c484499cd3fc6f76527c01b5` |

The index lists `zynq-bm3-am2` (S19, S19 Pro, S19j, S19j Pro, S19 XP, T19 and others) from 23.12 onwards.
There is no SD asset for it: only `transitional_*` (SSH install) and `sysupgrade_nand_*`. SD images
(`sysupgrade_sd_am2_s17`) exist for the older `am2-s17` target, from 21.09 to 25.01.

The `braiins-toolbox` binaries the user supplied contain no firmware. The toolbox downloads from the
index above.

## Identity

- DT model `Antminer X19 Miner Control Board`. `/lib/zynq.sh` maps it to board name `zynq-bm3-am2`.
- The bitstream header names the part `7z007sclg225`, a single-core Zynq. This explains
  `CPU1: failed to boot` in the stock log (`miner-41-boot-log.md`).
- LEDE 17.01.4. Kernel `4.4.0-xilinx`, built with LEDE GCC 5.4.0. U-Boot 2016.03.
- `memory` node: 256 MiB at 0. The DDR controller is set to a 16-bit bus (`ddrc_ctrl` = `0x84` in the
  SPL tables, see below).

## What the SSH package installs (`install.sh`, GPL-3.0-or-later)

The package keeps the stock NAND layout. It refuses to run unless the SHA-256 of `/proc/mtd` is
`7aca7b8a…`, which is the layout in `miner-41-boot-log.md`. It then:

1. Writes `system.bit.gz` (the Braiins bitstream) to mtd4 `reserve1` at offset 0. It backs up the
   stock image from mtd0 `0x2000000` (length `0x800000`), gzipped, to `reserve1` at `0x100000`.
2. Writes the factory image (`factory.tar.gz`) to mtd7 `reserve2`: a size header at `0x3a00000` and
   the image at `0x3b00000`. The post-install tarball goes to `0x100000`/`0x200000`.
3. Erases the U-Boot environments at mtd0 `0x1800000` (8 blocks). It writes an empty environment
   image to `0x18a0000`, then sets `ethaddr`, `miner_hwid` and the network settings in it with
   `fw_setenv`. The `fw_env` config gives two copies, at mtd0 `0x1880000` and `0x18a0000`, each
   `0x20000` long.
4. Overwrites mtd0 `0x2000000` (64 blocks) with `uImage.itb` and reboots.

The stock first stage therefore loads its "kernel" from mtd0 `0x2000000`. The Braiins `uImage.itb` is a
FIT whose kernel image is the **Braiins U-Boot** (load and entry `0x04000000`), followed by a recovery
FIT with a kernel, a squashfs ramdisk and a device tree. The stock U-Boot starts it as if it were Linux,
and the Braiins U-Boot takes over from there.

## Device tree (`factory/kernel`, FIT `fdt@1`)

The PS part is a standard Zynq-7000 tree. Board-specific facts:

- Console: `serial@e0000000` (UART0), 115200 8N1.
- Ethernet: `ethernet@e000b000`, GMII. SD: `mmc@e0100000` with card detect and write protect.
  NAND: `pl353`, `nand-ecc-mode = "none"`. Watchdog: `cdns,wdt-r1p2`, 10 s.
- PS GPIO (MIO): green LED 36 and red LED 35, both active high. The reset button is on MIO 15 and the
  IP-report button on MIO 37, both active low.
- A third LED, "Red LED (inside)", is pin 4 of the PL GPIO at `0x41210000`.

PL map. This is valid **only with the Braiins bitstream loaded**:

| Node | Address | IRQ (SPI) |
|---|---|---|
| `chainN-common` | `0x43C00000 + (N-1)*0x10000` | none |
| `chainN-cmd-rx` | `… + 0x1000` | chain 1–4: 32, 35, 53, 56 |
| `chainN-work-rx` | `… + 0x2000` | 31, 34, 52, 55 |
| `chainN-work-tx` | `… + 0x3000` | 30, 33, 36, 54 (on chain 4 the property is misspelled `nterrupts`, so Linux sees no IRQ) |
| `fan-control` | `0x42800000` | none |
| `board-control` | `0x42810000` | none |
| `miner-glitch-monitor` | `0x43D00000` | none |
| AXI GPIO, 4 inputs | `0x41200000` | none |
| AXI GPIO, 5 outputs (`dout-default` `0x10`), second channel 32 bits | `0x41210000` | none |
| AXI GPIO, 2 lines, interrupt capable | `0x41220000` | 57 |
| AXI IIC (alias `i2c0`) | `0x41600000` | 29 |
| ns16550 UART ×4 | `0x41001000 + n*0x10000` | 30, 33, 36, 54 |

Every `chain*`, `fan-control`, `board-control` and `glitch` node is `generic-uio`. The four ns16550
UARTs list the same interrupt numbers as the `work-tx` blocks. Which of the two descriptions matches
the bitstream is not known.

The `am2-s17` device tree (from the SD package) is **byte-for-byte the same apart from the model
string** ("Antminer S17 Miner Control Board").

**This is a different PL design from the stock Bitmain one.** VNish and the stock firmware use
Bitmain's bitstream with a register window at `0x40000000` (`vnish-fpga-base.md`). The map above
belongs to the Braiins bitstream. DCENT_OS drives the same Braiins design through UIO
(`hashboard-driver.md`, `oss-leads-2026-10-10.md`).

## Kernel (`factory/kernel`, FIT `kernel@1`)

`IKCONFIG` is present. Relevant options: `BLK_DEV_INITRD`, `RD_GZIP`, `RD_XZ`, `BLK_DEV_RAM`,
`SQUASHFS`, `VFAT_FS`, `MMC_SDHCI_OF_ARASAN`, `MACB`, `XILINX_DEVCFG`, `UIO`, `UIO_PDRV_GENIRQ`,
`I2C_XILINX`, `GPIO_XILINX`, `SERIAL_8250` and `DEVMEM`. `FPGA` (fpga_manager) is not set. The
string `generic-uio` is in the image, so the UIO nodes above probably bind without a command-line
parameter. Not verified.

## Bitstream

`system.bit.gz` has SHA-256 `7102a9f204a4be3edfbf2096f68e6a9b24b1a2845c8257e29563ce459a597c8b`. It is
the Xilinx `system_wrapper` design for `7z007sclg225`, dated 2022/11/28. The file is **identical** in the
`zynq-bm3-am2` 26.09.1 package and in the `am2-s17` 23.10.1 SD package.

Before `fpga loadb`, the Braiins U-Boot unlocks the SLCR (`0xF8000008` = `0xDF0D`), sets
`FPGA0_CLK_CTRL` (`0xF8000170`) to `0x00400400`, and locks the SLCR again (`0xF8000004` = `0x767B`).

## Braiins U-Boot environment (both builds)

- `bootcmd` runs `$modeboot`. On Zynq U-Boot, `modeboot` comes from the boot-mode pins (`sdboot` or
  `nandboot`).
- `sdboot` reads three files from the first FAT partition of the card:
  - `uEnv.txt`, imported into the environment.
  - `system.bit.gz`, unzipped to `0x2100000` and loaded with `fpga loadb`.
  - `fit.itb`, loaded at `0x2000000` and started with `bootm`.
- If `uEnv.txt` sets `sd_uenvcmd`, that command runs before the FPGA step. If it sets `bootargs`, those
  are used. Otherwise the default is `console=ttyPS0,115200 root=/dev/ram0 r rootfstype=squashfs …`.
- `sdboot` writes nothing to NAND. The NAND paths do: `nandboot_init`, `bootloaders_repair`,
  `uenv_reset` and `auto_recovery` erase or save to NAND.
- GPIO numbers in the environment: `led_green=36`, `led_red=35`, `button_ip=37`.

## am2-s17 SD package (`sysupgrade-am2-s17/`)

| File | Content |
|---|---|
| `spl` | Zynq BootROM image (`0xAA995566` "XNLX" at `0x20`, payload `0x131c7` bytes at `0x8c0`). U-Boot SPL 2016.03-g8728fee00a, 2020-12-05. This is the `BOOT.bin` for SD boot. |
| `uboot` | `u-boot.img`, "U-Boot 2016.03-gcaca2524a4", load and entry `0x04000000`, for "Antminer S17 Miner Control Board". Has the same `sdboot` as above. |
| `fpga` | Same bitstream as `zynq-bm3-am2` (hash above). |
| `fit` | Kernel 4.4.92, squashfs ramdisk "am2-s17", DT identical to X19. |

The SPL contains the `ps7_init` tables (`EMIT_MASKWRITE`, opcode `0x33`) for three silicon revisions:
PLL, clock, DDR, MIO and peripherals. A dump is in the scratchpad, not here.

The public `braiins/u-boot-xlnx` (`bos-release`) has `ps7_init_gpl.c` only for `am1_s9` and `dm1_*`. Not
for am2 or X19. `braiins/braiins-open` contains only the stratum and proxy crates.

## bosminer (proprietary, not analysed further)

- A Rust binary of 23.6 MB. Its embedded source paths are under `open/bosminer/bosminer-am2-s17/src/hashchain/`,
  with one module per chip: `bm1398`, `bm1397`, `bm1396`, `bm1362`, `bm1366`, `bm1368`, `bm1370`.
  That source is not public.
- It uses `/dev/uio*`, `/sys/class/uio/`, `/dev/i2c-0`, `/dev/i2c-1`, `/dev/gpiochip*`,
  `/sys/class/gpio/` and `/dev/mem`.
- Line names in the binary: `HB0_PLUG`…`HB3_PLUG`, `HB1_RESET`…`HB3_RESET`, `PWR_CONTROL`,
  `PWR_I2C2_SDA` and `PWR_I2C2_SCL`. The PSU is probably reached over a bit-banged I2C. Not verified.

## Awesome 1.3.5 (`awesome-s19-xil-nand-v1.3.5.tar`, supplied by the user)

- A rebranded VNish build (FIT "AnthillOS Image", `fw_name` "Awesome", `build_name` "awesome").
- `runme.sh` is identical to VNish 1.3.5. The `cgminer_btm` (`9f9b64d5`) and `libbitmain` (`1a52239`)
  commits match too.
- `hwscan` differs as a file, but the FPGA logical-ID table is byte-identical to the one in VNish
  (offset `0x471544` here, `0x4719c4` in VNish). Two independent builds agree on the table.
- `.41` runs the VNish "Boost" build (`build_time` 2026-07-30 21:45:49), not Awesome.

## Consequences for OpenMinerOS

1. **SD first stage.** The am2-s17 SD package has the piece the SD tests lacked: a `BOOT.bin` (SPL)
   and a `u-boot.img` built for a 7z007s Antminer board with this device tree. See
   `sd-test-plan.md`, "Third test". Both are third-party binaries: use them only for local tests,
   never ship them.
2. **Our own first stage.** The SPL's `ps7_init` tables are the board's PS configuration. With them,
   an SPL can be built from mainline U-Boot sources for this board. That gives a first stage we can
   distribute.
3. **Two driver targets.**
   - The Bitmain stock PL at `0x40000000`, as used by VNish. Its register map is partly recovered.
   - The Braiins PL at `0x43C00000`, which has the map above and an open reference driver (DCENT_OS).
     Its bitstream is proprietary and cannot be shipped.
   `crates/board-xil` must state which of the two it targets.
