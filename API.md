# API

All public REST endpoints are versioned under `/api/v1`.

## System

- `GET /api/v1/system/info`
- `GET /api/v1/system/firmware`
- `GET /api/v1/system/health`

`/system/firmware` returns the identity written by the image build: name,
version, board, install type (`sd`, `nand`, `emmc`, `development`), build time,
build id and source commit.

## Mining

- `GET /api/v1/miner/status`
- `GET /api/v1/chains`
- `GET /api/v1/fans`
- `GET /api/v1/pools`

Chain state is one of `initializing`, `mining`, `stopped`, `disabled`,
`disconnected`, `fault`. Each chain reports detected and expected ASIC count,
hashrate, frequency, voltage and hardware errors.

Fan state is `ok` or `lost`.

Every pool the miner talks to is listed with an `owner` of `operator` or
`contribution`, so contribution mining is always visible next to the
operator's own pools.

`power_measured` in `/miner/status` tells whether `power_w` comes from PSU
telemetry or is an estimate.

## Contribution

- `GET /api/v1/contribution/status`

The default contribution rate is `0.0`. Operators may explicitly choose any
rate from `0.0` to `3.0`.

Contribution configuration is public by design. The project does not hide,
obfuscate, or silently redirect contribution mining.

Default opt-in contribution target:

```json
{
  "beneficiary": "<beneficiary-address>",
  "endpoints": [
    "stratum+tcp://ss.antpool.com:3333",
    "stratum+tcp://ss.antpool.com:443",
    "stratum+tcp://ss.antpool.com:25"
  ]
}
```

The endpoint list is inactive while contribution is disabled.

## Metrics

- `GET /metrics`

Metrics use the `omo_` prefix.
