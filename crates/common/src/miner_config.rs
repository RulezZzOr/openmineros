//! Persistent miner configuration shared by `omo-minerd` and the control plane.

use serde::{Deserialize, Serialize};

pub const CONFIG_PATH: &str = "/data/omo/config.json";
pub const STATUS_PATH: &str = "/tmp/omo-status.json";

/// Hardware limits of the S19 / APW121215a combination (from bosminer's
/// own ranges on the unit).
pub const MIN_VOLTAGE: f64 = 11.96;
pub const MAX_VOLTAGE: f64 = 15.14;
pub const MIN_FREQ_MHZ: f64 = 50.0;
pub const MAX_FREQ_MHZ: f64 = 700.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoolConfig {
    /// `host:port`, optionally prefixed with `stratum+tcp://`.
    pub url: String,
    pub user: String,
    #[serde(default = "default_password")]
    pub password: String,
}

fn default_password() -> String {
    "x".into()
}

impl PoolConfig {
    /// `host:port` without the scheme.
    pub fn host_port(&self) -> String {
        self.url
            .trim()
            .trim_start_matches("stratum+tcp://")
            .trim_end_matches('/')
            .to_owned()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FanMode {
    Auto,
    Manual,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FanConfig {
    pub mode: FanMode,
    /// Auto mode: hottest PCB sensor target (°C).
    pub target_temp: f64,
    /// Auto mode floor and manual mode value (percent).
    pub min_percent: u32,
    pub manual_percent: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MinerConfig {
    pub pools: Vec<PoolConfig>,
    pub freq_mhz: f64,
    pub voltage: f64,
    pub fan: FanConfig,
    /// Hottest PCB sensor that stops mining and switches power off (°C).
    pub max_pcb_temp: f64,
}

/// Named operating points from Braiins OS autotuner profiles saved on two
/// plain S19 units (`/etc/bosminer-autotune.json` on .41 and .48, average chip
/// clock per profile; Braiins' own power estimate). 800 W and 1500 W are
/// interpolated. Our firmware runs one clock per chain, Braiins tunes per chip.
pub fn presets() -> Vec<(&'static str, f64, f64, &'static str)> {
    vec![
        ("400 W", 92.0, 11.96, "Braiins profile, ~13 TH/s"),
        ("500 W", 120.0, 11.96, "Braiins profile, ~17 TH/s"),
        (
            "600 W",
            148.0,
            11.96,
            "Braiins profile, ~21 TH/s (23.6 TH/s on .48)",
        ),
        ("700 W", 177.0, 11.96, "Braiins profile, ~25 TH/s"),
        ("800 W", 205.0, 11.96, "interpolated, 28.9 TH/s measured"),
        ("900 W", 233.0, 11.96, "Braiins profile, ~33 TH/s"),
        ("1000 W", 261.0, 11.96, "Braiins profile, ~37 TH/s"),
        ("1100 W", 290.0, 11.96, "Braiins profile, ~41 TH/s"),
        ("1200 W", 318.0, 11.96, "Braiins profile, ~45 TH/s"),
        (
            "1400 W",
            375.0,
            11.96,
            "Braiins profile, 51.2 TH/s measured",
        ),
        ("1500 W", 395.0, 12.06, "interpolated, ~56 TH/s"),
        ("2000 W", 494.0, 12.57, "Braiins profile, ~70 TH/s"),
        ("stock 3250 W", 675.0, 13.8, "factory S19 point, ~95 TH/s"),
    ]
}

/// Estimated wall power (W), fitted to the Braiins profiles above:
/// 74 W + 3.54 W/MHz × f × (V / 11.96)².
pub fn estimated_power(freq_mhz: f64, voltage: f64) -> f64 {
    74.0 + 3.54 * freq_mhz * (voltage / 11.96).powi(2)
}

impl Default for MinerConfig {
    fn default() -> Self {
        Self {
            pools: vec![],
            freq_mhz: 109.0,
            voltage: 11.96,
            fan: FanConfig {
                mode: FanMode::Auto,
                target_temp: 50.0,
                min_percent: 30,
                manual_percent: 60,
            },
            max_pcb_temp: 80.0,
        }
    }
}

impl MinerConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.pools.is_empty() {
            return Err("at least one pool is required".into());
        }
        if self.pools.len() > 3 {
            return Err("at most three pools".into());
        }
        for (i, p) in self.pools.iter().enumerate() {
            let hp = p.host_port();
            let Some((host, port)) = hp.rsplit_once(':') else {
                return Err(format!("pool {}: url must be host:port", i + 1));
            };
            if host.is_empty() || port.parse::<u16>().is_err() {
                return Err(format!("pool {}: bad host or port", i + 1));
            }
            if p.user.trim().is_empty() {
                return Err(format!("pool {}: user is empty", i + 1));
            }
        }
        if !(MIN_FREQ_MHZ..=MAX_FREQ_MHZ).contains(&self.freq_mhz) {
            return Err(format!(
                "frequency must be {MIN_FREQ_MHZ}–{MAX_FREQ_MHZ} MHz"
            ));
        }
        if !(MIN_VOLTAGE..=MAX_VOLTAGE).contains(&self.voltage) {
            return Err(format!("voltage must be {MIN_VOLTAGE}–{MAX_VOLTAGE} V"));
        }
        // Guard from the Braiins profiles: up to ~420 MHz runs at the PSU
        // minimum; above that the chips need more voltage.
        let need = if self.freq_mhz > 550.0 {
            13.0
        } else if self.freq_mhz > 420.0 {
            12.3
        } else {
            MIN_VOLTAGE
        };
        if self.voltage + 1e-9 < need {
            return Err(format!("{:.0} MHz needs at least {need} V", self.freq_mhz));
        }
        if !(20.0..=75.0).contains(&self.fan.target_temp) {
            return Err("fan target must be 20–75 °C".into());
        }
        if self.fan.min_percent > 100
            || self.fan.manual_percent > 100
            || self.fan.manual_percent < 20
        {
            return Err("fan percent must be 20–100".into());
        }
        if !(40.0..=90.0).contains(&self.max_pcb_temp) {
            return Err("max PCB temperature must be 40–90 °C".into());
        }
        Ok(())
    }

    pub fn load(path: &str) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let c: Self = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;
        c.validate()?;
        Ok(c)
    }

    /// Writes atomically (temp file + rename).
    pub fn save(&self, path: &str) -> Result<(), String> {
        self.validate()?;
        if let Some(dir) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let tmp = format!("{path}.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap())
            .map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> MinerConfig {
        MinerConfig {
            pools: vec![PoolConfig {
                url: "stratum+tcp://pool.example.com:3333".into(),
                user: "a.b".into(),
                password: "x".into(),
            }],
            ..Default::default()
        }
    }

    #[test]
    fn validates_ranges_and_pools() {
        assert!(cfg().validate().is_ok());
        assert_eq!(cfg().pools[0].host_port(), "pool.example.com:3333");
        let mut c = cfg();
        c.freq_mhz = 675.0;
        assert!(c.validate().is_err(), "675 MHz at 11.96 V must be refused");
        c.freq_mhz = 318.0;
        assert!(c.validate().is_ok(), "Braiins runs 318 MHz at 11.96 V");
        c.freq_mhz = 675.0;
        c.voltage = 13.8;
        assert!(c.validate().is_ok());
        c.voltage = 16.0;
        assert!(c.validate().is_err());
        let mut c = cfg();
        c.pools.clear();
        assert!(c.validate().is_err());
    }

    #[test]
    fn power_model_matches_profiles() {
        assert!((estimated_power(148.0, 11.96) - 598.0).abs() < 15.0);
        assert!((estimated_power(318.0, 11.96) - 1200.0).abs() < 15.0);
        assert!((estimated_power(494.0, 12.57) - 2000.0).abs() < 30.0);
    }

    #[test]
    fn presets_pass_validation() {
        for (_, f, v, _) in presets() {
            let mut c = cfg();
            c.freq_mhz = f;
            c.voltage = v;
            assert!(c.validate().is_ok(), "{f} MHz {v} V");
        }
    }
}
