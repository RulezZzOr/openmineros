use crate::UpdateError;
use openmineros_common::BoardFamily;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Component, Path},
};

pub const MANIFEST_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRole {
    Kernel,
    DeviceTree,
    Rootfs,
    Rescue,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Artifact {
    pub role: ArtifactRole,
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub name: String,
    pub version: String,
    pub board: BoardFamily,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_upgrade_from: Option<String>,
    pub artifacts: Vec<Artifact>,
}

impl Manifest {
    pub(crate) fn validate(&self) -> Result<(), UpdateError> {
        if self.schema != MANIFEST_SCHEMA {
            return Err(UpdateError::UnsupportedSchema(self.schema));
        }
        if self.artifacts.is_empty() {
            return Err(UpdateError::InvalidManifest(
                "manifest lists no artifacts".to_string(),
            ));
        }

        let mut paths = BTreeSet::new();
        let mut roles = BTreeSet::new();

        for artifact in &self.artifacts {
            if !is_plain_file_name(&artifact.path) {
                return Err(UpdateError::UnsafePath(artifact.path.clone()));
            }
            if !paths.insert(artifact.path.as_str()) {
                return Err(UpdateError::InvalidManifest(format!(
                    "artifact path {} is listed twice",
                    artifact.path
                )));
            }
            if !roles.insert(artifact.role) {
                return Err(UpdateError::InvalidManifest(format!(
                    "artifact role {:?} is listed twice",
                    artifact.role
                )));
            }
            if artifact.sha256.len() != 64
                || !artifact.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(UpdateError::InvalidManifest(format!(
                    "artifact {} has a malformed sha256",
                    artifact.path
                )));
            }
        }

        Ok(())
    }
}

/// Bundles are flat: an artifact path is a single file name, never a path
/// that could point outside the bundle directory or at bundle metadata.
fn is_plain_file_name(path: &str) -> bool {
    let mut components = Path::new(path).components();

    matches!(
        (components.next(), components.next()),
        (Some(Component::Normal(name)), None) if name.to_str() == Some(path)
    ) && path != crate::MANIFEST_FILE
        && path != crate::SIGNATURE_FILE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_plain_file_names() {
        assert!(is_plain_file_name("rootfs.img"));

        for path in [
            "",
            ".",
            "..",
            "./rootfs.img",
            "a/b",
            "/rootfs.img",
            "rootfs.img/",
            "manifest.json",
            "manifest.sig",
        ] {
            assert!(!is_plain_file_name(path), "{path:?} must be rejected");
        }
    }

    #[test]
    fn rejects_unknown_manifest_fields() {
        let json = r#"{
            "schema": 1,
            "name": "openmineros",
            "version": "0.2.0",
            "board": "xilinx",
            "post_install": "runme.sh",
            "artifacts": []
        }"#;

        assert!(serde_json::from_str::<Manifest>(json).is_err());
    }
}
