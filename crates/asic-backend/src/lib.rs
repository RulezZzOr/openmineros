use openmineros_common::{
    BoardFamily, BoardProfile, Capability, CapabilitySet, ChainState, ChainStatus, FanState,
    FanStatus, MinerMode, MinerStatus, Model, PoolOwner, PoolState, PoolStatus, SupportLevel,
    TargetError, supported_targets,
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BackendError {
    #[error(transparent)]
    Target(#[from] TargetError),
}

const SIMULATED_CHAINS: u8 = 3;
const SIMULATED_FANS: u8 = 4;

#[derive(Debug, Clone)]
pub struct SimulatedBackend {
    model: Model,
    profile: BoardProfile,
    support: SupportLevel,
}

impl SimulatedBackend {
    pub fn new(model: Model, board: BoardFamily) -> Result<Self, BackendError> {
        let support = supported_targets()
            .into_iter()
            .find(|target| target.model == model && target.board == board)
            .map(|target| target.support)
            .ok_or(TargetError::UnsupportedTarget { model, board })?;

        let profile = match board {
            BoardFamily::Xilinx => openmineros_board_xil::profile(),
            BoardFamily::BeagleBone => openmineros_board_bb::profile(),
            BoardFamily::Amlogic => openmineros_board_aml::profile(),
            BoardFamily::Cvitek => BoardProfile {
                family: BoardFamily::Cvitek,
                soc: "CV1835",
                recovery: "unsupported in 0.1.0",
                capabilities: CapabilitySet::from_flags([Capability::SafeMode]),
            },
        };

        Ok(Self {
            model,
            profile,
            support,
        })
    }

    pub fn model(&self) -> Model {
        self.model
    }

    pub fn profile(&self) -> &BoardProfile {
        &self.profile
    }

    pub fn support(&self) -> SupportLevel {
        self.support
    }

    pub fn miner_status(&self) -> MinerStatus {
        let hashrate_ths = match self.model {
            Model::S19 => 95.0,
            Model::S19Pro => 110.0,
            Model::S19j => 90.0,
            Model::S19jPro => 104.0,
            Model::S19Xp => 141.0,
            Model::T19 => 84.0,
        };
        let power_w = match self.model {
            Model::S19Xp => 3010,
            Model::S19Pro => 3250,
            _ => 3068,
        };

        MinerStatus {
            hashrate_ths,
            hashrate_nominal_ths: hashrate_ths,
            power_w,
            power_measured: self
                .profile
                .capabilities
                .has(Capability::TelemetryPowerNative),
            efficiency_j_th: f64::from(power_w) / hashrate_ths,
            accepted_shares: 0,
            rejected_shares: 0,
            hw_errors: 0,
            restart_count: 0,
            mode: MinerMode::Balanced,
        }
    }

    pub fn chain_statuses(&self) -> Vec<ChainStatus> {
        let chain_hashrate_ths = self.miner_status().hashrate_ths / f64::from(SIMULATED_CHAINS);

        (0..SIMULATED_CHAINS)
            .map(|id| ChainStatus {
                id,
                state: ChainState::Mining,
                present: true,
                enabled: true,
                asic_detected: 42,
                asic_expected: 42,
                hashrate_ths: chain_hashrate_ths,
                frequency_mhz: 525.0,
                voltage_v: 13.2,
                hw_errors: 0,
                temp_board_c: 62.0 + f64::from(id),
                temp_chip_max_c: 78.0 + f64::from(id),
                fault: None,
            })
            .collect()
    }

    pub fn fan_statuses(&self) -> Vec<FanStatus> {
        (0..SIMULATED_FANS)
            .map(|id| FanStatus {
                id,
                state: FanState::Ok,
                rpm: 5800 + u32::from(id) * 20,
                pwm_percent: 70,
            })
            .collect()
    }

    pub fn pool_statuses(&self) -> Vec<PoolStatus> {
        vec![PoolStatus {
            priority: 0,
            url: "stratum+tcp://pool.example:3333".to_string(),
            user: "acct.worker".to_string(),
            owner: PoolOwner::Operator,
            state: PoolState::Active,
            accepted_shares: 0,
            rejected_shares: 0,
            stale_shares: 0,
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructs_all_mvp_backends() {
        for board in [
            BoardFamily::Xilinx,
            BoardFamily::BeagleBone,
            BoardFamily::Amlogic,
        ] {
            let backend = SimulatedBackend::new(Model::S19jPro, board).unwrap();
            assert_eq!(backend.profile().family, board);
            assert_eq!(backend.chain_statuses().len(), 3);
            assert_eq!(backend.fan_statuses().len(), 4);
        }
    }

    #[test]
    fn chain_hashrates_add_up_to_miner_hashrate() {
        let backend = SimulatedBackend::new(Model::S19jPro, BoardFamily::Xilinx).unwrap();
        let total: f64 = backend
            .chain_statuses()
            .iter()
            .map(|chain| chain.hashrate_ths)
            .sum();

        assert!((total - backend.miner_status().hashrate_ths).abs() < 1e-9);
    }

    #[test]
    fn simulated_pools_belong_to_the_operator() {
        let backend = SimulatedBackend::new(Model::S19jPro, BoardFamily::Xilinx).unwrap();

        assert!(
            backend
                .pool_statuses()
                .iter()
                .all(|pool| pool.owner == PoolOwner::Operator)
        );
    }
}
