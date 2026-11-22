# SD card test plan for the OpenMinerOS S19 Xilinx image

Status: **blocked on a kernel and device tree for the board.** Everything else
is ready. Written 2026-10-10 for a test on 2026-10-11.

## What the SD test can show

- The control board boots our ramdisk image, the control plane starts, and the
  local API on port 80 answers (`/api/v1/system/firmware`, `/api/v1/system/health`).
- `omo-fpga-probe --raw 0x40000000` and `--raw 0x43C00000` run on real hardware.
  Those are the two base-address candidates in `vnish-fpga-base.md`.
- Nothing hashes. The image reports `hashing: false` and
  `health = idle` with the issue "hashboard driver is not implemented".

## What the test cannot show

- Any mining, chain detection, or PSU control. The driver is not written.
- Anything about the BM1398 chip protocol. See `cgminer-chip-protocol.md`.

## Missing inputs (the blocker)

A Zynq SD boot needs three things we do not have:

| Input | Status |
|---|---|
| `BOOT.BIN` (first-stage loader, u-boot, FPGA bitstream) | Not in this repo. The stock images keep the boot chain in NAND. |
| Linux kernel for the board (4.6.0-xilinx, from the miner's own image) | Not in this repo. The VNish package only has the ramdisk (`uramdisk.image.gz`). |
| Device tree for the S19 Xilinx control board | Not verified. |

DCENT_OS's post-image script says the same thing: it refuses to package a
BM1398 image until "a verified am2-s19 kernel" exists
(`br2_external_dcentos/board/zynq/am2-s19pro/post-image.sh`).

Ways to get the inputs:

1. **Dump the kernel and DTB from a working unit's NAND.** This needs a shell on
   that unit. `.47` runs VNish and has SSH open, but we have no credentials.
2. **Use a public Zynq kernel and DTB.** Only usable after checking the board
   matches the device tree. An unverified kernel could leave the board in a
   state the stock boot does not recover from.
3. **Use the Braiins or DCENT SD image** for a comparable Zynq S19 board. Its
   behaviour on our board is untested.

## Which unit to use

- `192.168.2.41` (s19a, VNish) is offline and not mining. It is the natural
  choice for the SD test. Power it on, check that it answers, then start the test.
- `192.168.2.47` (s19c, VNish) is mining. Do not run the SD test on it.

## Safety rules for the test

- Do not write to NAND (`mtd*`) or to any register. The probe only reads.
- Set the boot-mode switch for SD only for the test, and put it back afterwards.
- Keep the PSU and fans as the stock firmware drives them. Our image does not
  control them.
- If the board does not boot, power off, remove the card, and check that the
  VNish NAND boot still works.

## Staged test folder (2026-10-12)

`~/omo-sd-test/` (outside the repository, not committed):

- `uImage`: kernel 4.6.0-xilinx, copied from the ANTHILLOS stick. Load and entry `0x8000`.
- `devicetree.dtb`: copied from the same stick (`xlnx,zynq-7000`).
- `uramdisk.image.gz`: OpenMinerOS ramdisk as a FIT image (the build's `.itb`),
  the same format as VNish's `uramdisk.image.gz`. The first card (2026-10-12) had
  the raw `ramdisk.cpio.gz`, which `bootm` does not accept as an initrd.
- `uEnv.txt`: boot arguments taken from the `.41` boot log
  (`console=ttyPS0,115200 mem=240M root=/dev/ram ramdisk_size=… earlyprintk`), and a
  `bootm` that loads kernel, ramdisk and DTB from the card.
- `SHA256SUMS`: checksums for all four files.

The kernel config (`IKCONFIG`) has SD (`MMC_SDHCI_OF_ARASAN`), `VFAT_FS`, `NLS_CODEPAGE_437`,
`BLK_DEV_INITRD` with `RD_GZIP`, and `XILINX_DEVCFG`.

## Boot log on the card (2026-10-13)

The ramdisk runs `omo-sdlog` at boot. It mounts the card and writes
`omo-logs/boot-N/` (`system.txt`, `status.log`, `dmesg.txt`, `ps.txt`, `fpga.txt`),
refreshed every 30 s. It reads only. If there is no DHCP lease after about 20 s, it adds
`192.168.2.241/24` to `eth0`. After one minute it runs `omo-fpga-probe --raw` at both
base candidates, but only if devcfg `prog_done` is `1`; an empty `omo-no-probe` file on the
card disables the probe.

Reading the card afterwards tells which stage was reached:

- No `omo-logs/` directory: our kernel and ramdisk never ran. The first stage is the problem.
- `omo-logs/` exists: Linux and our init ran. The logs show what happened next.

The first test (2026-10-12, card without `BOOT.BIN`) left the card unchanged and `.41` never
appeared on the network.

The stick's `boot.bin` and `update.image.gz` are **not** staged. `boot.bin`
looks obfuscated and runs code we have not read. `update.image.gz` would
install over NAND.

## Remaining blocker: the first-stage loader

The `uEnv.txt` only works if the code that reads the card (u-boot in NAND or a
`BOOT.BIN`) runs first. Nothing staged so far provides that stage. Options:

1. **Test whether the miner's NAND u-boot already reads the card.** Insert the
   card and reboot `.41` (VNish is NAND-booted). If it boots our image, the
   first stage is solved. If not, nothing is changed. This needs your decision,
   because it reboots a miner.
2. **Build a `BOOT.BIN` from an open first stage** (FSBL and u-boot for Zynq
   7010). Not in the repository yet.

## Checklist for tomorrow

Steps 1 and 2 are already done in the staged folder.

1. Confirm the input files (`BOOT.BIN`, kernel, DTB) exist and their hashes are
   recorded. Without them, stop.
2. Build the ramdisk: `make ramdisk BOARD=s19-xil MODEL=s19`. Verify the hash
   against `dist/*-sha256sum.txt`.
3. Write the card (done by you): FAT partition with `BOOT.BIN`, kernel, DTB, and
   the ramdisk `.itb`.
4. Insert the card into the `.41` unit, power on, and wait for boot.
5. From a laptop on the same network: `curl http://<ip>/api/v1/system/firmware`
   and `curl http://<ip>/api/v1/system/health`.
6. If a shell is available: `omo-fpga-probe --raw 0x40000000` and
   `--raw 0x43C00000`. Save the JSON output.
7. Power off, remove the card, and confirm the unit returns to VNish boot.

## Result of the second test (2026-10-13)

Card: the files above (FIT ramdisk, `omo-sdlog`), freshly formatted FAT32, no `BOOT.BIN`.
`.41` was powered down at 13:32 and booted with the card. The fans kept speeding up and
slowing down, which suggests repeated resets. Nothing new appeared on the network: no
placeholder MAC `00:0a:35:00:00:00`, nothing at `.241`. When the card was read back on the Mac,
there was no `omo-logs/` and the four files were unchanged.

Conclusion: our init never ran, or the board reset within the first seconds of Linux.
The two cases cannot be told apart without the serial console (`ttyPS0`, 115200 8N1, 3.3 V)
or a known-good first stage. Next inputs: a USB-to-TTL 3.3 V adapter on the control
board's UART header, or the ANTHILLOS stick's `boot.bin`.

## Third test: Braiins am2 first stage (staged 2026-10-15, not run)

Both earlier cards had no `BOOT.BIN`, and with either card inserted `.41` did not reach VNish on
NAND. So inserting a card changes the boot path. One explanation fits both results: the BootROM
switches to SD when a card is present and then finds no first stage. Not proven.

The Braiins `am2-s17` SD package has an SPL `BOOT.bin` and a `u-boot.img` for a 7z007s Antminer board.
Its device tree and bitstream are identical to the X19 ones (`braiins-zynq-bm3-am2.md`). Staged in
`~/omo-sd-test3/` (outside the repository, with `SHA256SUMS`):

| File | Source |
|---|---|
| `BOOT.bin` | Braiins `am2-s17` 23.10.1 `spl` (U-Boot SPL 2016.03) |
| `u-boot.img` | Braiins `am2-s17` 23.10.1 `uboot` (U-Boot 2016.03) |
| `system.bit.gz` | Braiins bitstream, `7z007sclg225` |
| `fit.itb` | Braiins kernel 4.4.0-xilinx (26.09.1), our ramdisk, Braiins X19 device tree |
| `uEnv.txt` | `bootargs=console=ttyPS0,115200 earlyprintk`, `ethaddr=xx:xx:xx:xx:xx:xx` |
| `omo-no-probe` | Skips the FPGA probe (see below) |

The U-Boot `sdboot` path loads `uEnv.txt`, loads the bitstream, loads `fit.itb` and runs `bootm`. It
writes nothing to NAND. The Braiins kernel has no devtmpfs, so `rcS` now fills `/dev` with `mdev` when
devtmpfs is missing.

Checked in qemu (`xilinx-zynq-a9`, the same kernel and ramdisk, the device tree without `amba_pl`):

- Linux booted and `mdev` found the SD card.
- `omo-sdlog` wrote `omo-logs/boot-1/`, DHCP got a lease and the control plane answered on port 80.

With `amba_pl` present, qemu hangs in the AXI IIC probe because qemu has no PL. On the board, U-Boot
loads the bitstream before Linux, and if that step fails it does not start Linux at all.

`omo-no-probe` is on the card because, with the Braiins bitstream loaded, nothing may answer at
`0x40000000` (the stock base). A read there could stall the bus.

What to look for afterwards:

- A new host with MAC `xx:xx:xx:xx:xx:xx` on the network, or `.241`.
- `omo-logs/` on the card.
- In `system.txt`: `uio` should list the `chain*`, `fan-control` and `board-control` nodes, and
  `/proc/iomem` should show the PL ranges.

The fans and PSU stay as the bitstream leaves them, because our image does not drive them. Running
this test reboots `.41` and needs the user's go-ahead.

## Vendor SD card layout (Awesome 1.2.6, analysed 2026-10-15)

Public download `SD-awesome-s19-xil-sd-v1.2.6-install.zip` (17 MB, SHA-256 `bcdf9615…`), kept outside
the repository and not redistributed. Read-only analysis:

- **No MBR.** The image is one FAT16 volume starting at sector 1, label `ANTHILLOS`. Sector 0 is zero.
- **Files:** `boot.bin` (81 920 B, no `XNLX` header at `0x20`), `uEnv.txt`, `uImage` (4 057 312 B),
  `devicetree.dtb` (8 043 B), `update.image.gz`, `miner.btm`, and `.sig` files for the signed ones.
  The `uImage` and DTB sizes match the ANTHILLOS stick we already have.
- **`uEnv.txt`:** `uenvcmd=dcache off && fatload mmc 0 0x1000000 boot.bin && go 0x1000000`.
  So the card does not rely on BootROM for its second stage. It runs an unsigned command.
- **Stock U-Boot order (disassembly, unverified on hardware):** in the SD-select branch,
  `uenvboot` imports `uEnv.txt` and runs `uenvcmd` before the `fatload` + RSA-checked path.
  An `uenvcmd` that ends in `bootm` or `go` never reaches the signature checks.

Consequences:
- Test 2 used this mechanism: `uEnv.txt` with `uenvcmd` and `bootm`, unsigned files. Test 3 did
  not: its `uEnv.txt` had no `uenvcmd`, and it relied on the Braiins SPL, which the stock NAND
  U-Boot never runs. Test 3 therefore tested the wrong chain.
- Next test (not run): the card as the Awesome layout, with our kernel, DTB and ramdisk loaded
  by `uenvcmd` into RAM, and no NAND write.

## Test 4 result (2026-10-15)

Card: FAT32 superfloppy (no partition table) with `uImage` (stock stick kernel), `devicetree.dtb`
(stock stick DTB), `uramdisk.image.gz` (current OpenMinerOS FIT, SHA-256 `c48f8632…`) and `uEnv.txt`
with `uenvcmd` that loads and `bootm`s the three files, plus `omo-no-probe`. Written with `dd`
from a 1 GB image, checksums verified on the card afterwards.

Result on `.41`: the fans cycled up and down, as in test 2. Nothing answered on the network
(no ping, ports 22 and 80 closed, no fallback at `.241`). The card was not changed by the miner.

Conclusion: the fan loop is the same symptom as test 2, with a different card layout. Without a
serial console the stage where the board resets is still unknown. Tests 2–4 are stopped until
a UART is connected.
