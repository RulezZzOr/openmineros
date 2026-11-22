# Miner 192.168.2.47: web API facts (VNish 1.2.7)

Read-only, collected with GET requests from the miner's own web UI on
2026-10-10 about 20:34 CEST. Nothing was written to the miner.

## Endpoints that answered

The web bundle sets `baseURL: "/api/v1"`. Endpoints that returned JSON:

| Path | Content |
|---|---|
| `/api/v1/summary` | Miner state, hashrate, pools, boards (large object) |
| `/api/v1/status` | `miner_state`, `miner_state_time`, `find_miner`, `restart_required`, `reboot_required`, `unlocked` |
| `/api/v1/perf-summary` | Current preset, preset switcher settings |
| `/api/v1/info` | Model, firmware, build, platform, network, uptime |
| `/api/v1/chains` | Per-chain state, frequency, sensors, per-chip data |

Guessed paths (`fans`, `pools`, `psu`, `autotune`, `presets`, `system`,
`miner`, `version`, `system/info`) return 404. Fan and pool data are inside
`summary`, not separate endpoints.

## Identity

- `info.model` = `s19`, `info.fw_name` = `Vnish`, `info.fw_version` = `1.2.7`
- `info.platform` = `xil`, `install_type` = `nand`, `build_time` = `2025-12-22 19:41:26`
- `info.miner_name` = `s19c`. The UI title uses this. The API model field stays `s19`.
- `summary.miner.miner_type` = `Antminer S19 (Vnish 1.2.7)`
- `info.serial` = `N/A`

## Uptime and a possible shared event

- `info.system.uptime` = `0:24` at 20:34 CEST, so the unit booted about 20:10.
- `status.miner_state_time` = `1287` s, consistent with the restart.
- The other unit (`192.168.2.41`) first timed out at 20:20:41 and dropped off
  the network at about 20:23. Both events fall within roughly ten minutes.
  A shared cause (power, switch, or the router) is possible. **Not proven.**

## Chains

- Three chains (`id` 1 to 3), each with `freq` about 378 to 379 MHz and
  `status.state` = `mining`.
- Each chain lists 76 chips. The API does not expose a per-chip state field,
  so this view cannot show bad chips; the dashboard's "75 / 1" on board 2 is
  not visible here.
- Per-chip fields: `id`, `hr`, `freq`, `volt`, `temp`, `errs`, `grade`, `throttled`.
- On this build `volt` and `temp` are 0 for chip 1, and its `grade` is `grey`.
  Per-chip values for the other chips were not checked.
  So the API gives per-chip hashrate and frequency, not per-chip voltage or
  temperature.
- Each chain also has 4 `sensors` entries with `board`, `chip` and `loc` fields.

## Hashrate and preset

- `hr_realtime` per chain is about 17.6 to 17.9 TH/s, total about 53 TH/s.
- Preset `1100` (`1100 watt ~ 53 TH`), preset switcher disabled.

## Still unknown

- The chip model (BM13xx family). It is not in these responses; check the
  HWscan or Miner log, or the chip ID in `chains`.
- The mapping from `sensors.loc` to physical positions.
