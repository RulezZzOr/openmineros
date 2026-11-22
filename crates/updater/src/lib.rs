//! Verification of signed OpenMinerOS update bundles.
//!
//! A bundle is a flat directory with `manifest.json`, `manifest.sig` and the
//! artifacts the manifest lists. The manifest is purely declarative: a bundle
//! can never carry code that runs during installation. Nothing in the bundle
//! is trusted until the signature over the exact manifest bytes is verified.

mod manifest;
mod version;

pub use manifest::{Artifact, ArtifactRole, MANIFEST_SCHEMA, Manifest};
pub use version::{Version, VersionError};

use ed25519_dalek::{PUBLIC_KEY_LENGTH, SIGNATURE_LENGTH, Signature, VerifyingKey};
use openmineros_common::{BoardFamily, FirmwareInfo};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};
use thiserror::Error;

pub const MANIFEST_FILE: &str = "manifest.json";
pub const SIGNATURE_FILE: &str = "manifest.sig";

const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

#[derive(Debug, Error)]
pub enum UpdateError {
    #[error("cannot read {path}: {source}")]
    Io { path: PathBuf, source: io::Error },
    #[error("{0} is missing from the bundle")]
    Missing(&'static str),
    #[error("manifest is larger than {MAX_MANIFEST_BYTES} bytes")]
    ManifestTooLarge,
    #[error("no trusted release keys are configured")]
    NoTrustedKeys,
    #[error("invalid release key: {0}")]
    InvalidKey(String),
    #[error("malformed manifest signature")]
    MalformedSignature,
    #[error("manifest signature does not match any trusted release key")]
    UntrustedSignature,
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("unsupported manifest schema {0}")]
    UnsupportedSchema(u32),
    #[error("bundle targets board {bundle}, this miner is {device}")]
    BoardMismatch {
        bundle: BoardFamily,
        device: BoardFamily,
    },
    #[error(transparent)]
    Version(#[from] VersionError),
    #[error("upgrade to {target} requires at least {required}, running {running}")]
    UpgradePathUnsupported {
        target: Version,
        required: Version,
        running: Version,
    },
    #[error("downgrade from {running} to {target} is not allowed")]
    Downgrade { running: Version, target: Version },
    #[error("unsafe artifact path: {0}")]
    UnsafePath(String),
    #[error("artifact {0} is not a regular file")]
    NotRegularFile(String),
    #[error("artifact {path} has size {actual}, manifest says {expected}")]
    SizeMismatch {
        path: String,
        expected: u64,
        actual: u64,
    },
    #[error("artifact {0} does not match its sha256")]
    DigestMismatch(String),
    #[error("bundle contains a file the manifest does not list: {0}")]
    UnlistedFile(String),
}

/// Release keys this device accepts. Keys are baked into the image, so an
/// update cannot introduce its own signing key.
#[derive(Debug, Clone, Default)]
pub struct TrustedKeys {
    keys: Vec<VerifyingKey>,
}

impl TrustedKeys {
    pub fn from_hex<'a>(keys: impl IntoIterator<Item = &'a str>) -> Result<Self, UpdateError> {
        let keys = keys
            .into_iter()
            .map(|key| {
                let bytes: [u8; PUBLIC_KEY_LENGTH] = decode_hex(key)
                    .and_then(|bytes| bytes.try_into().ok())
                    .ok_or_else(|| UpdateError::InvalidKey(key.to_string()))?;
                VerifyingKey::from_bytes(&bytes)
                    .map_err(|_| UpdateError::InvalidKey(key.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self { keys })
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    fn verify(&self, message: &[u8], signature: &Signature) -> Result<(), UpdateError> {
        if self.keys.is_empty() {
            return Err(UpdateError::NoTrustedKeys);
        }

        self.keys
            .iter()
            .any(|key| key.verify_strict(message, signature).is_ok())
            .then_some(())
            .ok_or(UpdateError::UntrustedSignature)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UpdatePolicy {
    pub allow_downgrade: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedArtifact {
    pub role: ArtifactRole,
    pub path: PathBuf,
    pub size: u64,
}

/// A bundle whose signature, target and every artifact have been checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedBundle {
    pub manifest: Manifest,
    pub version: Version,
    pub artifacts: Vec<VerifiedArtifact>,
}

pub fn verify_bundle(
    dir: &Path,
    keys: &TrustedKeys,
    running: &FirmwareInfo,
    policy: UpdatePolicy,
) -> Result<VerifiedBundle, UpdateError> {
    let manifest_bytes = read_small(&dir.join(MANIFEST_FILE), MANIFEST_FILE)?;
    let signature_text = read_small(&dir.join(SIGNATURE_FILE), SIGNATURE_FILE)?;

    let signature = parse_signature(&signature_text)?;
    keys.verify(&manifest_bytes, &signature)?;

    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| UpdateError::InvalidManifest(error.to_string()))?;
    manifest.validate()?;

    if manifest.board != running.board {
        return Err(UpdateError::BoardMismatch {
            bundle: manifest.board,
            device: running.board,
        });
    }

    let version = check_version(&manifest, running, policy)?;
    let artifacts = manifest
        .artifacts
        .iter()
        .map(|artifact| verify_artifact(dir, artifact))
        .collect::<Result<Vec<_>, _>>()?;
    reject_unlisted_files(dir, &manifest)?;

    Ok(VerifiedBundle {
        manifest,
        version,
        artifacts,
    })
}

fn check_version(
    manifest: &Manifest,
    running: &FirmwareInfo,
    policy: UpdatePolicy,
) -> Result<Version, UpdateError> {
    let target: Version = manifest.version.parse()?;
    let current: Version = running.version.parse()?;

    if target < current && !policy.allow_downgrade {
        return Err(UpdateError::Downgrade {
            running: current,
            target,
        });
    }

    if let Some(required) = &manifest.min_upgrade_from {
        let required: Version = required.parse()?;
        if current < required {
            return Err(UpdateError::UpgradePathUnsupported {
                target,
                required,
                running: current,
            });
        }
    }

    Ok(target)
}

fn verify_artifact(dir: &Path, artifact: &Artifact) -> Result<VerifiedArtifact, UpdateError> {
    let path = dir.join(&artifact.path);
    let metadata = fs::symlink_metadata(&path).map_err(|source| UpdateError::Io {
        path: path.clone(),
        source,
    })?;

    if !metadata.file_type().is_file() {
        return Err(UpdateError::NotRegularFile(artifact.path.clone()));
    }
    if metadata.len() != artifact.size {
        return Err(UpdateError::SizeMismatch {
            path: artifact.path.clone(),
            expected: artifact.size,
            actual: metadata.len(),
        });
    }
    if !sha256_hex(&path)?.eq_ignore_ascii_case(&artifact.sha256) {
        return Err(UpdateError::DigestMismatch(artifact.path.clone()));
    }

    Ok(VerifiedArtifact {
        role: artifact.role,
        path,
        size: artifact.size,
    })
}

fn reject_unlisted_files(dir: &Path, manifest: &Manifest) -> Result<(), UpdateError> {
    let listed: BTreeSet<&str> = manifest
        .artifacts
        .iter()
        .map(|artifact| artifact.path.as_str())
        .chain([MANIFEST_FILE, SIGNATURE_FILE])
        .collect();
    let io_error = |source| UpdateError::Io {
        path: dir.to_path_buf(),
        source,
    };

    for entry in fs::read_dir(dir).map_err(io_error)? {
        let name = entry.map_err(io_error)?.file_name();
        let name = name.to_string_lossy();
        if !listed.contains(name.as_ref()) {
            return Err(UpdateError::UnlistedFile(name.into_owned()));
        }
    }

    Ok(())
}

fn read_small(path: &Path, name: &'static str) -> Result<Vec<u8>, UpdateError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(UpdateError::Missing(name));
        }
        Err(source) => {
            return Err(UpdateError::Io {
                path: path.to_path_buf(),
                source,
            });
        }
    };

    if !metadata.file_type().is_file() {
        return Err(UpdateError::Missing(name));
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err(UpdateError::ManifestTooLarge);
    }

    fs::read(path).map_err(|source| UpdateError::Io {
        path: path.to_path_buf(),
        source,
    })
}

fn parse_signature(text: &[u8]) -> Result<Signature, UpdateError> {
    let bytes: [u8; SIGNATURE_LENGTH] = std::str::from_utf8(text)
        .ok()
        .and_then(decode_hex)
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or(UpdateError::MalformedSignature)?;

    Ok(Signature::from_bytes(&bytes))
}

fn sha256_hex(path: &Path) -> Result<String, UpdateError> {
    let io_error = |source| UpdateError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut file = File::open(path).map_err(io_error)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];

    loop {
        let read = file.read(&mut buffer).map_err(io_error)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(encode_hex(&hasher.finalize()))
}

pub fn encode_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if !text.is_ascii() || text.len() % 2 != 0 {
        return None;
    }

    (0..text.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&text[index..index + 2], 16).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use openmineros_common::InstallType;
    use tempfile::TempDir;

    const ROOTFS: &[u8] = b"rootfs image";
    const KERNEL: &[u8] = b"kernel image";

    fn release_key() -> SigningKey {
        SigningKey::from_bytes(&[7; 32])
    }

    fn trusted(key: &SigningKey) -> TrustedKeys {
        TrustedKeys::from_hex([encode_hex(key.verifying_key().as_bytes()).as_str()]).unwrap()
    }

    fn running(version: &str) -> FirmwareInfo {
        FirmwareInfo {
            version: version.to_string(),
            install_type: InstallType::Nand,
            ..FirmwareInfo::development(BoardFamily::Xilinx)
        }
    }

    fn digest(bytes: &[u8]) -> String {
        encode_hex(&Sha256::digest(bytes))
    }

    fn manifest(version: &str) -> Manifest {
        Manifest {
            schema: MANIFEST_SCHEMA,
            name: "openmineros".to_string(),
            version: version.to_string(),
            board: BoardFamily::Xilinx,
            min_upgrade_from: None,
            artifacts: vec![
                Artifact {
                    role: ArtifactRole::Kernel,
                    path: "kernel.img".to_string(),
                    size: KERNEL.len() as u64,
                    sha256: digest(KERNEL),
                },
                Artifact {
                    role: ArtifactRole::Rootfs,
                    path: "rootfs.img".to_string(),
                    size: ROOTFS.len() as u64,
                    sha256: digest(ROOTFS),
                },
            ],
        }
    }

    fn write_bundle(manifest: &Manifest, key: &SigningKey) -> TempDir {
        let dir = TempDir::new().unwrap();
        let bytes = serde_json::to_vec_pretty(manifest).unwrap();

        fs::write(dir.path().join("kernel.img"), KERNEL).unwrap();
        fs::write(dir.path().join("rootfs.img"), ROOTFS).unwrap();
        fs::write(
            dir.path().join(SIGNATURE_FILE),
            encode_hex(&key.sign(&bytes).to_bytes()),
        )
        .unwrap();
        fs::write(dir.path().join(MANIFEST_FILE), bytes).unwrap();
        dir
    }

    fn verify(dir: &TempDir, running_version: &str) -> Result<VerifiedBundle, UpdateError> {
        verify_bundle(
            dir.path(),
            &trusted(&release_key()),
            &running(running_version),
            UpdatePolicy::default(),
        )
    }

    #[test]
    fn accepts_a_correctly_signed_bundle() {
        let dir = write_bundle(&manifest("0.2.0"), &release_key());
        let bundle = verify(&dir, "0.1.0").unwrap();

        assert_eq!(bundle.version.to_string(), "0.2.0");
        assert_eq!(bundle.artifacts.len(), 2);
        assert_eq!(bundle.artifacts[1].role, ArtifactRole::Rootfs);
    }

    #[test]
    fn rejects_a_bundle_signed_by_an_unknown_key() {
        let dir = write_bundle(&manifest("0.2.0"), &SigningKey::from_bytes(&[9; 32]));

        assert!(matches!(
            verify(&dir, "0.1.0"),
            Err(UpdateError::UntrustedSignature)
        ));
    }

    #[test]
    fn rejects_a_manifest_modified_after_signing() {
        let dir = write_bundle(&manifest("0.2.0"), &release_key());
        let path = dir.path().join(MANIFEST_FILE);
        let edited = fs::read_to_string(&path).unwrap().replace("0.2.0", "9.9.9");
        fs::write(path, edited).unwrap();

        assert!(matches!(
            verify(&dir, "0.1.0"),
            Err(UpdateError::UntrustedSignature)
        ));
    }

    #[test]
    fn refuses_to_verify_without_trusted_keys() {
        let dir = write_bundle(&manifest("0.2.0"), &release_key());
        let result = verify_bundle(
            dir.path(),
            &TrustedKeys::default(),
            &running("0.1.0"),
            UpdatePolicy::default(),
        );

        assert!(matches!(result, Err(UpdateError::NoTrustedKeys)));
    }

    #[test]
    fn rejects_a_tampered_artifact() {
        let dir = write_bundle(&manifest("0.2.0"), &release_key());
        fs::write(dir.path().join("rootfs.img"), b"rootfs imagE").unwrap();

        assert!(matches!(
            verify(&dir, "0.1.0"),
            Err(UpdateError::DigestMismatch(path)) if path == "rootfs.img"
        ));
    }

    #[test]
    fn rejects_a_truncated_artifact() {
        let dir = write_bundle(&manifest("0.2.0"), &release_key());
        fs::write(dir.path().join("kernel.img"), b"kernel").unwrap();

        assert!(matches!(
            verify(&dir, "0.1.0"),
            Err(UpdateError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn rejects_files_the_manifest_does_not_list() {
        let dir = write_bundle(&manifest("0.2.0"), &release_key());
        fs::write(dir.path().join("runme.sh"), b"#!/bin/sh\n").unwrap();

        assert!(matches!(
            verify(&dir, "0.1.0"),
            Err(UpdateError::UnlistedFile(name)) if name == "runme.sh"
        ));
    }

    #[test]
    fn rejects_a_bundle_for_another_board() {
        let mut manifest = manifest("0.2.0");
        manifest.board = BoardFamily::Amlogic;
        let dir = write_bundle(&manifest, &release_key());

        assert!(matches!(
            verify(&dir, "0.1.0"),
            Err(UpdateError::BoardMismatch { .. })
        ));
    }

    #[test]
    fn rejects_paths_that_escape_the_bundle() {
        for path in ["../rootfs.img", "/etc/shadow", "a/rootfs.img", "..", ""] {
            let mut manifest = manifest("0.2.0");
            manifest.artifacts[1].path = path.to_string();
            let dir = write_bundle(&manifest, &release_key());

            assert!(
                matches!(verify(&dir, "0.1.0"), Err(UpdateError::UnsafePath(_))),
                "{path:?} must be rejected"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn rejects_symlinked_artifacts() {
        let dir = write_bundle(&manifest("0.2.0"), &release_key());
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("rootfs.img");
        fs::write(&target, ROOTFS).unwrap();
        fs::remove_file(dir.path().join("rootfs.img")).unwrap();
        std::os::unix::fs::symlink(&target, dir.path().join("rootfs.img")).unwrap();

        assert!(matches!(
            verify(&dir, "0.1.0"),
            Err(UpdateError::NotRegularFile(_))
        ));
    }

    #[test]
    fn rejects_downgrade_unless_policy_allows_it() {
        let dir = write_bundle(&manifest("0.2.0"), &release_key());

        assert!(matches!(
            verify(&dir, "0.3.0"),
            Err(UpdateError::Downgrade { .. })
        ));

        let allowed = verify_bundle(
            dir.path(),
            &trusted(&release_key()),
            &running("0.3.0"),
            UpdatePolicy {
                allow_downgrade: true,
            },
        );
        assert!(allowed.is_ok());
    }

    #[test]
    fn enforces_minimum_upgrade_source() {
        let mut manifest = manifest("0.5.0");
        manifest.min_upgrade_from = Some("0.3.0".to_string());
        let dir = write_bundle(&manifest, &release_key());

        assert!(matches!(
            verify(&dir, "0.2.9"),
            Err(UpdateError::UpgradePathUnsupported { .. })
        ));
        assert!(verify(&dir, "0.3.0").is_ok());
    }

    #[test]
    fn rejects_missing_signature() {
        let dir = write_bundle(&manifest("0.2.0"), &release_key());
        fs::remove_file(dir.path().join(SIGNATURE_FILE)).unwrap();

        assert!(matches!(
            verify(&dir, "0.1.0"),
            Err(UpdateError::Missing(SIGNATURE_FILE))
        ));
    }
}
