# Build

Build `0.1.0` provides local development builds, tests, and non-flashable board artefacts.

## Requirements

- Rust toolchain
- `make`
- POSIX shell

## Commands

```bash
make bootstrap
make fmt
make clippy
make test
make run-control-plane BOARD=s19-xil MODEL=s19j-pro
make image BOARD=s19-xil MODEL=s19j-pro
make verify-repro BOARD=s19-xil MODEL=s19j-pro
```

## Supported Board Values

- `s19-xil`
- `s19-bb`
- `s19-aml`
- `s19-cvitek`

Only `s19-xil`, `s19-bb`, and `s19-aml` are part of the `0.1.0` MVP artefact flow.


## Ramdisk Image (s19-xil)

```bash
make ramdisk BOARD=s19-xil MODEL=s19j-pro
```

Needs Docker. The builder image (`image/builder.Dockerfile`) cross-compiles the
runtime for `armv7-unknown-linux-musleabihf`, adds a static BusyBox and the
files in `image/rootfs/`, and packs a gzip cpio ramdisk into a FIT image.

Outputs in `dist/`:

- `*-ramdisk.itb` – FIT image holding the ramdisk
- `*-ramdisk.cpio.gz` – the bare ramdisk
- `*-ramdisk-manifest.json` – what was built, from which commit
- `omo-fpga-probe-armv7` – static read-only FPGA probe, see
  [docs/research/hashboard-driver.md](docs/research/hashboard-driver.md)

The image boots to the local API and reports `idle`. It does not drive
hashboards, fans or the PSU, and it has not been booted on real hardware.
There is no installer yet: nothing in this repository writes to flash.
