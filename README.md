# OpenMinerOS

OpenMinerOS is an open mining OS prototype for Antminer S19-class hardware.

The project is published as GPL-3.0-only open source.

Build `0.1.0` is a safe development baseline:

- Rust workspace bootstrapped for runtime modules.
- Board identity support for Xilinx/Zynq, BeagleBone Black, Amlogic, and CVitek.
- S19-class model matrix with explicit support levels.
- Simulated ASIC backend for local API and UI development.
- REST endpoints for system info, firmware identity, health, miner status, chains, fans, pools, contribution status, and Prometheus metrics.
- Signed update bundle verification (Ed25519 manifest signature, SHA-256 artifacts), see [UPGRADE.md](UPGRADE.md).
- Placeholder reproducible image artefact flow for `s19-xil`, `s19-bb`, and `s19-aml`.
- Optional, transparent development contribution defaults to `0.0 %` and is capped at `3.0 %`.

- Real ramdisk image for the Xilinx control board (`make ramdisk`), see [BUILD.md](BUILD.md).
- Read-only FPGA probe `omo-fpga-probe` for Zynq control boards.

This build does **not** flash hardware or ship low-level ASIC drivers yet.
An image running on a miner reports `idle` and never simulated hashrate.

## Quick Start

```bash
make test
make run-control-plane BOARD=s19-xil MODEL=s19j-pro
```

Open `http://127.0.0.1:8080` for the local status UI.

## Board Builds

```bash
make image BOARD=s19-xil MODEL=s19j-pro
make image BOARD=s19-bb MODEL=s19j-pro
make image BOARD=s19-aml MODEL=s19j-pro
make verify-repro BOARD=s19-xil MODEL=s19j-pro
```

The generated files are metadata-only development artefacts in `dist/`.
They are intentionally not flashable firmware images.

## API

- `GET /api/v1/system/info`
- `GET /api/v1/system/firmware`
- `GET /api/v1/system/health`
- `GET /api/v1/miner/status`
- `GET /api/v1/chains`
- `GET /api/v1/fans`
- `GET /api/v1/pools`
- `GET /api/v1/contribution/status`
- `GET /metrics`

## Development Contribution

OpenMinerOS does not ship a mandatory devfee. The optional contribution target
is public and auditable in [DEVFEE.md](DEVFEE.md). Hidden wallet addresses,
obfuscated pool URLs, and silent redirects are out of scope for this project.

## Support

OpenMinerOS is free, open source (GPL-3.0-only) and has no dev fee. If it mines for you,
you can buy the developers a pizza: <https://buymeacoffee.com/cloudpeakify>
