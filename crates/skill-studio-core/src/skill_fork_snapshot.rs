//! Durable snapshot data; validated callers still supply operation and deployment authority.
use crate::{
    skill_backup_reservation::{BackupCopyLimits, ExistingBackup, ReservedBackup},
    skill_coordination::CancellationToken,
    skill_deployment::parse_deployment_id,
    skill_dotagents_ledger::DotagentsDetachIntent,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::ffi::OsStr;

const RECORD_NAME: &str = "fork-snapshots.json";
const MAX_RECORD_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkSnapshotReference {
    version: u32,
    operation_id: String,
    deployment_id: String,
    receipt_digest: String,
}

impl ForkSnapshotReference {
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn deployment_id(&self) -> &str {
        &self.deployment_id
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || !crate::skill_backup_reservation::valid_id(&self.operation_id)
            || parse_deployment_id(&self.deployment_id).is_none()
            || !self
                .receipt_digest
                .strip_prefix("sha256:")
                .is_some_and(|hash| {
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
        {
            return Err("Invalid fork snapshot reference".into());
        }
        Ok(())
    }
}

fn receipt_digest(bytes: &[u8]) -> String {
    let digest: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{digest}")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkSnapshotIdentities {
    pub live_tree: String,
    pub upstream_tree: String,
    pub provider_lock: String,
    pub provider_manifest: String,
    pub registry_before: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkSnapshotReceipt {
    version: u32,
    operation_id: String,
    deployment_id: String,
    identities: ForkSnapshotIdentities,
    detach: String,
}

impl ForkSnapshotReceipt {
    pub fn publish(
        backup: &ReservedBackup<'_>,
        deployment_id: &str,
        detach: &DotagentsDetachIntent,
        identities: ForkSnapshotIdentities,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<ForkSnapshotReference, String> {
        let receipt = Self {
            version: 2,
            operation_id: backup.operation_id().into(),
            deployment_id: deployment_id.into(),
            identities,
            detach: detach.to_record_json()?,
        };
        receipt.validate(backup.operation_id(), deployment_id)?;
        let bytes = serde_json::to_vec(&receipt).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_RECORD_BYTES {
            return Err("Fork snapshot record exceeds its limit".into());
        }
        for (name, identity) in receipt.entries() {
            backup
                .verify_entry(OsStr::new(name), identity, limits, cancellation)
                .map_err(|error| error.to_string())?;
        }
        if !receipt.registry_before_present() {
            backup
                .verify_absent("registry-before")
                .map_err(|error| error.to_string())?;
        }
        receipt.validate_provider_documents(
            &backup
                .read_record("provider-lock", 8 * 1024 * 1024)
                .map_err(|error| error.to_string())?,
            &backup
                .read_record("provider-manifest", 8 * 1024 * 1024)
                .map_err(|error| error.to_string())?,
        )?;
        backup
            .write_new_file(RECORD_NAME, &bytes)
            .map_err(|error| error.to_string())?;
        Ok(ForkSnapshotReference {
            version: 1,
            operation_id: receipt.operation_id,
            deployment_id: receipt.deployment_id,
            receipt_digest: receipt_digest(&bytes),
        })
    }

    pub fn read(
        backup: &ExistingBackup<'_>,
        reference: &ForkSnapshotReference,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, String> {
        reference.validate()?;
        if reference.operation_id != backup.operation_id() {
            return Err("Fork snapshot reference names another operation".into());
        }
        if cancellation.is_cancelled() {
            return Err("Fork snapshot read cancelled".into());
        }
        let bytes = backup
            .read_record(RECORD_NAME, MAX_RECORD_BYTES)
            .map_err(|error| error.to_string())?;
        if receipt_digest(&bytes) != reference.receipt_digest {
            return Err("Fork snapshot receipt does not match its saved digest".into());
        }
        let receipt: Self = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        receipt.validate(backup.operation_id(), &reference.deployment_id)?;
        for (name, identity) in receipt.entries() {
            backup
                .verify_entry(OsStr::new(name), identity, limits, cancellation)
                .map_err(|error| error.to_string())?;
        }
        if !receipt.registry_before_present() {
            backup
                .verify_absent("registry-before")
                .map_err(|error| error.to_string())?;
        }
        receipt.validate_provider_documents(
            &backup
                .read_record("provider-lock", 8 * 1024 * 1024)
                .map_err(|error| error.to_string())?,
            &backup
                .read_record("provider-manifest", 8 * 1024 * 1024)
                .map_err(|error| error.to_string())?,
        )?;
        if backup
            .read_record(RECORD_NAME, MAX_RECORD_BYTES)
            .map_err(|error| error.to_string())?
            != bytes
        {
            return Err("Fork snapshot record changed".into());
        }
        if cancellation.is_cancelled() {
            return Err("Fork snapshot read cancelled".into());
        }
        Ok(receipt)
    }

    /// Creates a new working base in caller-owned staging; never replaces an existing entry.
    pub fn copy_upstream_base(
        backup: &ExistingBackup<'_>,
        reference: &ForkSnapshotReference,
        staging: &crate::skill_backup_reservation::BackupStateRoot,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<crate::skill_backup_reservation::BackupCopyReport, String> {
        let receipt = Self::read(backup, reference, limits, cancellation)?;
        let report = backup
            .copy_verified_tree_to(
                OsStr::new("upstream-tree"),
                &receipt.identities.upstream_tree,
                staging,
                OsStr::new("base"),
                limits,
                cancellation,
            )
            .map_err(|error| error.to_string())?;
        Self::read(backup, reference, limits, cancellation)?;
        Ok(report)
    }

    pub fn detach(&self) -> Result<DotagentsDetachIntent, String> {
        DotagentsDetachIntent::from_record_json(&self.detach)
    }

    fn validate_provider_documents(&self, lock: &[u8], manifest: &[u8]) -> Result<(), String> {
        let state = self.detach()?.observe(
            Some(std::str::from_utf8(lock).map_err(|error| error.to_string())?),
            Some(std::str::from_utf8(manifest).map_err(|error| error.to_string())?),
        )?;
        if state != crate::skill_dotagents_ledger::DotagentsDetachState::Attached {
            return Err("Saved provider documents do not match pre-detach entries".into());
        }
        Ok(())
    }

    #[cfg(feature = "event-store")]
    pub(crate) fn upstream_identity(&self) -> &str {
        &self.identities.upstream_tree
    }

    pub fn registry_before_present(&self) -> bool {
        self.identities.registry_before.is_some()
    }

    fn entries(&self) -> Vec<(&str, &str)> {
        let mut entries = vec![
            ("live-tree", self.identities.live_tree.as_str()),
            ("upstream-tree", self.identities.upstream_tree.as_str()),
            ("provider-lock", self.identities.provider_lock.as_str()),
            (
                "provider-manifest",
                self.identities.provider_manifest.as_str(),
            ),
        ];
        if let Some(identity) = &self.identities.registry_before {
            entries.push(("registry-before", identity));
        }
        entries
    }

    fn validate(&self, operation_id: &str, deployment_id: &str) -> Result<(), String> {
        let deployment = parse_deployment_id(deployment_id).ok_or("Invalid snapshot deployment")?;
        if self.version != 2
            || self.operation_id != operation_id
            || self.deployment_id != deployment_id
            || deployment.scope != "global"
            || self.detach()?.name() != deployment.name
            || self.entries().iter().any(|(_, identity)| {
                !identity.strip_prefix("tree-v1:").is_some_and(|hash| {
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
            })
        {
            return Err("Fork snapshot receipt does not match its operation".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        skill_backup_reservation::BackupStateRoot,
        skill_deployment::{deployment_id, SkillDestination},
    };
    use cap_std::fs::Dir;
    use std::fs;

    #[test]
    fn durable_snapshot_receipt_reopens_and_rejects_mismatched_or_changed_inputs() {
        for change in [
            "none",
            "operation",
            "deployment",
            "version",
            "provider",
            "live",
            "upstream",
            "record-link",
            "coherent",
            "reference",
            "provider-lock",
            "provider-manifest",
            "registry-before",
            "provider-at-publication",
            "absent-registry",
            "unexpected-registry",
            "cancel",
        ] {
            let temp = tempfile::tempdir().unwrap();
            let source = temp.path().join("source");
            for name in ["live-tree", "upstream-tree"] {
                fs::create_dir_all(source.join(name)).unwrap();
                fs::write(source.join(name).join("SKILL.md"), name).unwrap();
            }
            fs::write(
                source.join("provider-lock"),
                "[skills.alpha]\nsource = 'owner/repo'",
            )
            .unwrap();
            fs::write(
                source.join("provider-manifest"),
                "[[skills]]\nname = 'alpha'\nsource = 'owner/repo'",
            )
            .unwrap();
            let registry_absent = matches!(change, "absent-registry" | "unexpected-registry");
            if !registry_absent {
                fs::write(source.join("registry-before"), "{}").unwrap();
            }
            if change == "provider-at-publication" {
                fs::write(
                    source.join("provider-lock"),
                    "[skills.alpha]\nsource = 'other/repo'",
                )
                .unwrap();
            }
            let input = Dir::open_ambient_dir(&source, cap_std::ambient_authority()).unwrap();
            let state = temp.path().join("state");
            fs::create_dir(&state).unwrap();
            let limits = BackupCopyLimits {
                max_bytes: 1024,
                max_entries: 10,
                max_depth: 5,
            };
            let selected = deployment_id(
                "alpha",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                &temp.path().join("home/.agents/skills/alpha"),
            );
            let detach = DotagentsDetachIntent::from_documents(
                "alpha",
                "[skills.alpha]\nsource = 'owner/repo'",
                "[[skills]]\nname = 'alpha'\nsource = 'owner/repo'",
            )
            .unwrap();
            let reference = {
                let root = BackupStateRoot::bind(&state).unwrap();
                let operation = root.reserve("fork").unwrap();
                let copy = |name| {
                    operation
                        .copy_entry(
                            &input,
                            OsStr::new(name),
                            OsStr::new(name),
                            limits,
                            &CancellationToken::default(),
                        )
                        .unwrap()
                };
                let identities = ForkSnapshotIdentities {
                    live_tree: copy("live-tree").tree_identity,
                    upstream_tree: copy("upstream-tree").tree_identity,
                    provider_lock: copy("provider-lock").tree_identity,
                    provider_manifest: copy("provider-manifest").tree_identity,
                    registry_before: (!registry_absent)
                        .then(|| copy("registry-before").tree_identity),
                };
                let published = ForkSnapshotReceipt::publish(
                    &operation,
                    &selected,
                    &detach,
                    identities.clone(),
                    limits,
                    &CancellationToken::default(),
                );
                if change == "provider-at-publication" {
                    assert!(published.is_err());
                    assert!(!state.join("backups/fork").join(RECORD_NAME).exists());
                    continue;
                }
                let reference = published.unwrap();
                assert!(ForkSnapshotReceipt::publish(
                    &operation,
                    &selected,
                    &detach,
                    identities.clone(),
                    limits,
                    &CancellationToken::default()
                )
                .is_err());
                reference
            };
            let reference: ForkSnapshotReference =
                serde_json::from_slice(&serde_json::to_vec(&reference).unwrap()).unwrap();
            let record_path = state.join("backups/fork").join(RECORD_NAME);
            let mut record: serde_json::Value =
                serde_json::from_slice(&fs::read(&record_path).unwrap()).unwrap();
            let original_record = record.clone();
            let token = CancellationToken::default();
            match change {
                "operation" => record["operation_id"] = serde_json::json!("other"),
                "deployment" => record["deployment_id"] = serde_json::json!("other"),
                "version" => record["version"] = serde_json::json!(3),
                "provider" => {
                    record["detach"] = serde_json::json!(detach
                        .to_record_json()
                        .unwrap()
                        .replace("alpha", "other"))
                }
                "live" => {
                    fs::write(state.join("backups/fork/live-tree/SKILL.md"), "changed").unwrap()
                }
                "upstream" => {
                    fs::write(state.join("backups/fork/upstream-tree/SKILL.md"), "changed").unwrap()
                }
                "coherent" => {
                    fs::write(state.join("backups/fork/live-tree/SKILL.md"), "changed").unwrap();
                    let directory = Dir::open_ambient_dir(
                        state.join("backups/fork"),
                        cap_std::ambient_authority(),
                    )
                    .unwrap();
                    let changed = crate::skill_backup_copy::inspect_entry(
                        &directory,
                        OsStr::new("live-tree"),
                        limits,
                        &CancellationToken::default(),
                    )
                    .unwrap();
                    record["identities"]["live_tree"] = serde_json::json!(changed.tree_identity);
                }
                "provider-lock" | "provider-manifest" | "registry-before" => {
                    fs::write(state.join("backups/fork").join(change), "changed").unwrap()
                }
                "unexpected-registry" => {
                    fs::write(state.join("backups/fork/registry-before"), "{}").unwrap()
                }
                "cancel" => token.cancel(),
                _ => {}
            }
            if record != original_record {
                fs::write(&record_path, serde_json::to_vec(&record).unwrap()).unwrap();
            }
            if change == "record-link" {
                let target = temp.path().join("record");
                fs::rename(&record_path, &target).unwrap();
                std::os::unix::fs::symlink(&target, &record_path).unwrap();
            }
            let root = BackupStateRoot::bind(&state).unwrap();
            let existing = root.open_existing("fork").unwrap();
            let mut reference = reference;
            if change == "reference" {
                reference.operation_id = "other".into();
            }
            let result = ForkSnapshotReceipt::read(&existing, &reference, limits, &token);
            assert_eq!(
                result.is_ok(),
                matches!(change, "none" | "absent-registry"),
                "{change}"
            );
            let staging = temp.path().join("working-base");
            fs::create_dir(&staging).unwrap();
            let output = BackupStateRoot::bind(&staging).unwrap();
            let copied = ForkSnapshotReceipt::copy_upstream_base(
                &existing, &reference, &output, limits, &token,
            );
            assert_eq!(
                copied.is_ok(),
                matches!(change, "none" | "absent-registry"),
                "export/{change}"
            );
            if let Ok(receipt) = result {
                assert_eq!(receipt.detach().unwrap().name(), "alpha");
                assert_eq!(
                    copied.unwrap().tree_identity,
                    receipt.identities.upstream_tree
                );
                assert_eq!(
                    fs::read(staging.join("base/SKILL.md")).unwrap(),
                    b"upstream-tree"
                );
                assert!(ForkSnapshotReceipt::copy_upstream_base(
                    &existing, &reference, &output, limits, &token
                )
                .is_err());
                fs::write(staging.join("base/SKILL.md"), "mutable working base").unwrap();
                assert_eq!(
                    fs::read(state.join("backups/fork/upstream-tree/SKILL.md")).unwrap(),
                    b"upstream-tree"
                );
                ForkSnapshotReceipt::read(&existing, &reference, limits, &token).unwrap();
                let protected_output = BackupStateRoot::bind(&state.join("backups/fork")).unwrap();
                assert!(ForkSnapshotReceipt::copy_upstream_base(
                    &existing,
                    &reference,
                    &protected_output,
                    limits,
                    &token
                )
                .is_err());
                assert!(!state.join("backups/fork/base").exists());
            } else {
                assert!(
                    !staging.join("base").exists(),
                    "export wrote invalid source/{change}"
                );
            }
        }
    }
}
