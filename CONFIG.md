# Configuration

Build `0.1.0` keeps configuration schema work small and explicit.

## Contribution

```toml
[contribution]
enabled = false
rate_percent = 0.0
beneficiary = "<your-btc-address>"

[[contribution.endpoints]]
url = "stratum+tcp://ss.antpool.com:3333"
user = "<your-btc-address>.openmineros"
password = "x"

[[contribution.endpoints]]
url = "stratum+tcp://ss.antpool.com:443"
user = "<your-btc-address>.openmineros"
password = "x"

[[contribution.endpoints]]
url = "stratum+tcp://ss.antpool.com:25"
user = "<your-btc-address>.openmineros"
password = "x"
```

Validation rules:

- `rate_percent` must be between `0.0` and `3.0`,
- `enabled = false` means no contribution mining,
- endpoint and beneficiary values must be visible through UI/API,
- production mode must reject hidden or obfuscated contribution targets.

