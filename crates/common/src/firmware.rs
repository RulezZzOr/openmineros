use crate::BoardFamily;
use serde::{Deserialize, Serialize};
use std::{fs, io, path::Path};
use thiserror::Error;

pub const DEFAULT_FIRMWARE_INFO_PATH: &str = "/etc/openmineros/fw-info.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallType {
    Sd,
    Nand,
    Emmc,
    Development,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirmwareInfo {
    pub name: String,
    pub version: String,
    pub board: BoardFamily,
    pub install_type: InstallType,
    #[serde(default)]
    pub build_time: Option<String>,
    #[serde(default)]
    pub build_id: Option<String>,
    #[serde(default)]
    pub source_commit: Option<String>,
}

#[derive(Debug, Error)]
pub enum FirmwareInfoError {
    #[error("cannot read firmware info: {0}")]
    Io(#[from] io::Error),
    #[error("invalid firmware info: {0}")]
    Parse(#[from] serde_json::Error),
}

impl FirmwareInfo {
    pub fn development(board: BoardFamily) -> Self {
        Self {
            name: "OpenMinerOS".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            board,
            install_type: InstallType::Development,
            build_time: None,
            build_id: None,
            source_commit: None,
        }
    }

    pub fn from_json(json: &str) -> Result<Self, FirmwareInfoError> {
        Ok(serde_json::from_str(json)?)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, FirmwareInfoError> {
        Self::from_json(&fs::read_to_string(path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_image_firmware_info() {
        let info = FirmwareInfo::from_json(
            r#"{
                "name": "OpenMinerOS",
                "version": "0.2.0",
                "board": "xilinx",
                "install_type": "nand",
                "build_time": "2026-09-27T12:00:00Z",
                "source_commit": "00b2887"
            }"#,
        )
        .unwrap();

        assert_eq!(info.board, BoardFamily::Xilinx);
        assert_eq!(info.install_type, InstallType::Nand);
        assert_eq!(info.build_id, None);
    }

    #[test]
    fn development_info_is_never_a_flash_install() {
        let info = FirmwareInfo::development(BoardFamily::Amlogic);

        assert_eq!(info.install_type, InstallType::Development);
        assert_eq!(info.version, env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn rejects_unknown_board() {
        assert!(
            FirmwareInfo::from_json(
                r#"{"name":"x","version":"1","board":"stm","install_type":"nand"}"#
            )
            .is_err()
        );
    }
}
