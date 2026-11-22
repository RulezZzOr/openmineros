use openmineros_asic_backend::{BackendError, SimulatedBackend};
use openmineros_common::status::HealthStatusResponse;
use openmineros_common::{
    BoardFamily, ChainStatus, ContributionConfig, ContributionStatus, FanStatus, FirmwareInfo,
    HealthStatus, InstallType, MinerMode, MinerStatus, Model, PoolStatus, Severity, SystemInfo,
};
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct Supervisor {
    backend: SimulatedBackend,
    firmware: FirmwareInfo,
    booted_at: Instant,
    active_slot: String,
    contribution: ContributionConfig,
}

impl Supervisor {
    pub fn new(model: Model, board: BoardFamily) -> Result<Self, BackendError> {
        Self::with_firmware(model, FirmwareInfo::development(board))
    }

    pub fn with_firmware(model: Model, firmware: FirmwareInfo) -> Result<Self, BackendError> {
        Ok(Self {
            backend: SimulatedBackend::new(model, firmware.board)?,
            firmware,
            booted_at: Instant::now(),
            active_slot: "slot_a".to_string(),
            contribution: ContributionConfig::default(),
        })
    }

    pub fn system_info(&self) -> SystemInfo {
        SystemInfo {
            model: self.backend.model(),
            board_family: self.backend.profile().family,
            firmware_version: self.firmware.version.clone(),
            active_slot: self.active_slot.clone(),
            uptime_seconds: self.booted_at.elapsed().as_secs(),
            serial: None,
            capabilities: self.backend.profile().capabilities.clone(),
        }
    }

    /// Simulated telemetry is only ever served by development builds. An
    /// image running on a miner must never report hashrate it does not have.
    fn simulated(&self) -> bool {
        self.firmware.install_type == InstallType::Development
    }

    pub fn health(&self) -> HealthStatusResponse {
        if !self.simulated() {
            return HealthStatusResponse {
                state: HealthStatus::Idle,
                severity: Severity::Warn,
                issues: vec![
                    "hashboard driver is not implemented in this build, the miner is not hashing"
                        .to_string(),
                ],
                active_slot: self.active_slot.clone(),
                rollback_available: false,
            };
        }

        let support = self.backend.support();
        let mut issues = Vec::new();
        let severity = match support {
            openmineros_common::SupportLevel::MvpStable => Severity::Ok,
            openmineros_common::SupportLevel::Experimental => {
                issues.push("target is experimental in build 0.1.0".to_string());
                Severity::Warn
            }
            openmineros_common::SupportLevel::Unsupported => {
                issues.push("target is unsupported in build 0.1.0".to_string());
                Severity::Error
            }
        };

        HealthStatusResponse {
            state: HealthStatus::Mining,
            severity,
            issues,
            active_slot: self.active_slot.clone(),
            rollback_available: true,
        }
    }

    pub fn miner_status(&self) -> MinerStatus {
        if !self.simulated() {
            return MinerStatus {
                hashrate_ths: 0.0,
                hashrate_nominal_ths: 0.0,
                power_w: 0,
                power_measured: false,
                efficiency_j_th: 0.0,
                accepted_shares: 0,
                rejected_shares: 0,
                hw_errors: 0,
                restart_count: 0,
                mode: MinerMode::SafeMode,
            };
        }

        self.backend.miner_status()
    }

    pub fn chains(&self) -> Vec<ChainStatus> {
        if !self.simulated() {
            return Vec::new();
        }

        self.backend.chain_statuses()
    }

    pub fn firmware(&self) -> &FirmwareInfo {
        &self.firmware
    }

    pub fn fans(&self) -> Vec<FanStatus> {
        if !self.simulated() {
            return Vec::new();
        }

        self.backend.fan_statuses()
    }

    pub fn pools(&self) -> Vec<PoolStatus> {
        if !self.simulated() {
            return Vec::new();
        }

        self.backend.pool_statuses()
    }

    pub fn contribution_status(&self) -> ContributionStatus {
        ContributionStatus::from(self.contribution)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openmineros_common::InstallType;

    #[test]
    fn system_info_reports_the_image_firmware_version() {
        let firmware = FirmwareInfo {
            version: "0.2.0".to_string(),
            install_type: InstallType::Nand,
            ..FirmwareInfo::development(BoardFamily::Xilinx)
        };
        let supervisor = Supervisor::with_firmware(Model::S19jPro, firmware).unwrap();

        assert_eq!(supervisor.system_info().firmware_version, "0.2.0");
        assert_eq!(supervisor.system_info().board_family, BoardFamily::Xilinx);
    }

    #[test]
    fn installed_image_never_reports_simulated_hashrate() {
        let firmware = FirmwareInfo {
            install_type: InstallType::Nand,
            ..FirmwareInfo::development(BoardFamily::Xilinx)
        };
        let supervisor = Supervisor::with_firmware(Model::S19jPro, firmware).unwrap();

        assert_eq!(supervisor.health().state, HealthStatus::Idle);
        assert_eq!(supervisor.health().severity, Severity::Warn);
        assert_eq!(supervisor.miner_status().hashrate_ths, 0.0);
        assert!(supervisor.chains().is_empty());
        assert!(supervisor.fans().is_empty());
        assert!(supervisor.pools().is_empty());
    }

    #[test]
    fn development_build_serves_simulated_telemetry() {
        let supervisor = Supervisor::new(Model::S19jPro, BoardFamily::Xilinx).unwrap();

        assert_eq!(supervisor.health().state, HealthStatus::Mining);
        assert_eq!(supervisor.chains().len(), 3);
    }
}
