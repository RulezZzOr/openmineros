# Miner 192.168.2.41: stock NAND boot log (2026-10-13)

Kernel log of the `.41` unit (s19a) booting from NAND without an SD card, pasted
by the user from the miner on 2026-10-13. The unit booted about 12:48 CEST and was
mining 22 minutes later. Facts only, nothing was written to the miner.

## Identity

- `/api/v1/info`: `fw_name` = `Boost`, `fw_version` = `1.3.5`, `platform` = `xil`,
  `install_type` = `nand`, `build_time` = `2026-07-30 21:45:49`, `miner_name` = `s19a`.
- Kernel `4.6.0-xilinx-g03c746f7`, built 2021-12-20. The kernel on the ANTHILLOS stick
  (staged in `~/omo-sd-test/uImage`) is a different build: `g77a5f591`, 2020-05-25.
- Initramfs 15852 KiB, which matches the VNish 1.3.5 `uramdisk.image.gz` (15850.76 KiB).
- Command line: `mem=240M console=ttyPS0,115200 ramdisk_size=33554432 root=/dev/ram rw earlyprintk`.
- `CPU1: failed to boot: -1`. The board runs on one Cortex-A9 core.

## NAND layout (Micron MT29F2G08ABAEAWP, 256 MiB, 128 KiB erase blocks)

| mtd | Range | Name |
|---|---|---|
| 0 | `0x00000000-0x02800000` | `BOOT.bin-dts-marker-kernel` |
| 1 | `0x02800000-0x04800000` | `ramfs` |
| 2 | `0x04800000-0x05000000` | `configs` |
| 3 | `0x05000000-0x05200000` | `sig` |
| 4 | `0x05200000-0x06000000` | `reserve1` |
| 5 | `0x06000000-0x07000000` | `upgrade-ramfs` |
| 6 | `0x07000000-0x0a800000` | `upgrade-file` (UBI, UBIFS volume `upgrade-file`) |
| 7 | `0x0a800000-0x10000000` | `reserve2` |

The first stage (`BOOT.bin`, with FSBL, bitstream and u-boot) lives in mtd0. The
kernel warns that the controller's 1-bit ECC is weaker than the 4-bit/512 B the
chip requires.

## Ethernet

- `macb` at `0xe000b000`, Generic PHY, link 100/Full.
- The kernel registers `eth0` with `00:0a:35:00:00:00`, the Xilinx placeholder from the
  device tree's `local-mac-address`. VNish userland later sets the real MAC
  `xx:xx:xx:xx:xx:xx`. The source of the real MAC is unknown.
- Our ramdisk does not set a MAC. Booted from SD with the stick's device tree, it
  comes up as `00:0a:35:00:00:00` and gets a different DHCP lease than `.41`.

## What the log does not show

- No `mmcblk0` line: no SD card was inserted for this boot.
- No FPGA, UIO or `fpga_manager` line. The bitstream is presumably loaded by the
  first stage before Linux starts. Not verified.
