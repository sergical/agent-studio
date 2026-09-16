use super::*;
use crate::skill_backup_reservation::{valid_id, ExistingBackup};
use crate::skill_fork_transition::UnforkRegistryState;
use sha2::{Digest, Sha256};
use std::ffi::OsStr;

const RECORD: &str = "unfork-before.json";
const MAX_RECORD: usize = 24 * 1024 * 1024;

/// A provider-specific snapshot reference. It leaves the persisted Dotagents
/// V2 receipt and its `unfork-before.json` shape unchanged.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsShUnforkSnapshotReference {
    version: u32,
    operation_id: String,
    deployment_id: String,
    receipt_digest: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SkillsShUnforkSnapshotReceipt {
    version: u32,
    operation_id: String,
    pub(super) selection: UnforkRegistryTransition,
    pub(super) request: crate::skill_backup_reservation::SkillsShReinstallRequest,
    pub(super) live_identity: String,
    document_identities: [String; 2],
}

impl SkillsShUnforkSnapshotReference {
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }
    pub fn deployment_id(&self) -> &str {
        &self.deployment_id
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.version != 1
            || !valid_id(&self.operation_id)
            || crate::skill_deployment::parse_deployment_id(&self.deployment_id).is_none()
            || !valid_digest(&self.receipt_digest, "sha256:")
        {
            return Err("Invalid skills.sh Unfork snapshot reference".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnforkSnapshotReference {
    version: u32,
    operation_id: String,
    deployment_id: String,
    receipt_digest: String,
}

impl UnforkSnapshotReference {
    pub fn operation_id(&self) -> &str {
        &self.operation_id
    }

    pub fn deployment_id(&self) -> &str {
        &self.deployment_id
    }

    pub(crate) fn version(&self) -> u32 {
        self.version
    }

    pub fn validate(&self) -> Result<(), String> {
        if !matches!(self.version, 1 | 2)
            || !valid_id(&self.operation_id)
            || crate::skill_deployment::parse_deployment_id(&self.deployment_id).is_none()
            || !valid_digest(&self.receipt_digest, "sha256:")
        {
            return Err("Invalid Unfork snapshot reference".into());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnforkSnapshotReceipt {
    version: u32,
    operation_id: String,
    source_event_id: String,
    selection: UnforkRegistryTransition,
    original_rows: String,
    live_identity: String,
    document_identities: [String; 3],
    #[serde(default, skip_serializing_if = "Option::is_none")]
    v2: Option<UnforkSnapshotV2>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct UnforkSnapshotV2 {
    request: DotagentsReinstallRequest,
}

fn digest(bytes: &[u8]) -> String {
    let hex: String = Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("sha256:{hex}")
}

fn valid_digest(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

impl UnforkSnapshotReceipt {
    pub fn source_event_id(&self) -> &str {
        &self.source_event_id
    }

    pub fn selection(&self) -> &UnforkRegistryTransition {
        &self.selection
    }

    pub fn v2_request(&self) -> Option<&DotagentsReinstallRequest> {
        self.v2.as_ref().map(|v2| &v2.request)
    }

    pub fn live_identity(&self) -> &str {
        &self.live_identity
    }

    pub fn read(
        backup: &ExistingBackup<'_>,
        reference: &UnforkSnapshotReference,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, String> {
        reference.validate()?;
        if cancellation.is_cancelled() || backup.operation_id() != reference.operation_id {
            return Err("Unfork snapshot operation changed or read cancelled".into());
        }
        let bytes = backup
            .read_record(RECORD, MAX_RECORD)
            .map_err(|error| error.to_string())?;
        if digest(&bytes) != reference.receipt_digest {
            return Err("Unfork snapshot receipt digest changed".into());
        }
        let receipt: Self = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        receipt.selection.validate()?;
        let v2 = receipt.version == 2
            && reference.version == 2
            && receipt.selection.is_bound_current()
            && receipt.source_event_id.is_empty()
            && receipt.original_rows.is_empty()
            && receipt.v2.as_ref().is_some_and(|v2| {
                DotagentsReinstallRequest::from_fork_record(
                    receipt.selection.recorded_provenance(),
                    receipt.selection.name(),
                )
                .is_ok_and(|request| request == v2.request)
            });
        if !v2
            || receipt.operation_id != reference.operation_id
            || receipt.selection.record().deployment_id != reference.deployment_id
            || !valid_digest(&receipt.live_identity, "tree-v1:")
            || receipt
                .document_identities
                .iter()
                .any(|id| !valid_digest(id, "tree-v1:"))
        {
            return Err("Unfork snapshot parts do not identify one operation".into());
        }
        for (name, identity) in [
            ("live-tree", &receipt.live_identity),
            ("provider-lock", &receipt.document_identities[0]),
            ("provider-manifest", &receipt.document_identities[1]),
            ("registry-before", &receipt.document_identities[2]),
        ] {
            backup
                .verify_entry(OsStr::new(name), identity, limits, cancellation)
                .map_err(|error| error.to_string())?;
        }
        let lock = backup
            .read_record("provider-lock", 8 * 1024 * 1024)
            .map_err(|error| error.to_string())?;
        let manifest = backup
            .read_record("provider-manifest", 8 * 1024 * 1024)
            .map_err(|error| error.to_string())?;
        receipt
            .v2_request()
            .ok_or("V2 Unfork snapshot request is missing")?
            .validate_detached_documents(
                std::str::from_utf8(&lock).map_err(|error| error.to_string())?,
                std::str::from_utf8(&manifest).map_err(|error| error.to_string())?,
            )?;
        let registry = backup
            .read_record("registry-before", 8 * 1024 * 1024)
            .map_err(|error| error.to_string())?;
        if receipt.selection.observe_document(&registry)? != UnforkRegistryState::Before {
            return Err("Unfork backup does not contain its selected Fork owner".into());
        }
        if backup
            .read_record(RECORD, MAX_RECORD)
            .map_err(|error| error.to_string())?
            != bytes
            || cancellation.is_cancelled()
        {
            return Err("Unfork snapshot changed or read cancelled".into());
        }
        backup.revalidate().map_err(|error| error.to_string())?;
        Ok(receipt)
    }
}

impl PreparedDotagentsUnfork<'_> {
    pub fn publish_before_snapshot(
        &self,
        store: &EventStore,
        operation_id: &str,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<UnforkSnapshotReference, String> {
        self.revalidate(store, limits, cancellation)?;
        if !valid_id(operation_id) {
            return Err("Unfork requires a new operation ID".into());
        }
        let state = BackupStateRoot::bind(&store.app_data).map_err(|error| error.to_string())?;
        let backup = state
            .reserve(operation_id)
            .map_err(|error| error.to_string())?;
        let live = backup
            .copy_entry(
                &self.live.directory,
                &self.live.name,
                OsStr::new("live-tree"),
                limits,
                cancellation,
            )
            .map_err(|error| error.to_string())?;
        if live.tree_identity != self.live_identity {
            return Err("Unfork live tree changed during backup".into());
        }
        let agents = self
            .selection
            .record()
            .skill_dir
            .parent()
            .and_then(|path| path.parent())
            .ok_or("Unfork agents root missing")?;
        let root = BackupSourceRoot::bind(agents).map_err(|error| error.to_string())?;
        let mut document_identities = [String::new(), String::new(), String::new()];
        for (index, (name, target, expected)) in [
            ("agents.lock", "provider-lock", &self.provider_lock),
            ("agents.toml", "provider-manifest", &self.provider_manifest),
            ("skill-studio.json", "registry-before", &self.registry),
        ]
        .into_iter()
        .enumerate()
        {
            self.revalidate(store, limits, cancellation)?;
            let source = root
                .select(OsStr::new(name))
                .map_err(|error| error.to_string())?;
            let report = backup
                .copy_entry(
                    &source.directory,
                    &source.name,
                    OsStr::new(target),
                    limits,
                    cancellation,
                )
                .map_err(|error| error.to_string())?;
            source.revalidate().map_err(|error| error.to_string())?;
            if &backup
                .read_record(target, 8 * 1024 * 1024)
                .map_err(|error| error.to_string())?
                != expected
            {
                return Err("Unfork document changed during backup".into());
            }
            document_identities[index] = report.tree_identity;
        }
        let receipt = UnforkSnapshotReceipt {
            version: 2,
            operation_id: operation_id.into(),
            source_event_id: String::new(),
            selection: self.selection.clone(),
            original_rows: String::new(),
            live_identity: self.live_identity.clone(),
            document_identities,
            v2: Some(UnforkSnapshotV2 {
                request: self.reinstall.clone(),
            }),
        };
        let bytes = serde_json::to_vec(&receipt).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_RECORD {
            return Err("Unfork snapshot receipt exceeds its limit".into());
        }
        self.revalidate(store, limits, cancellation)?;
        backup
            .write_new_file(RECORD, &bytes)
            .map_err(|error| error.to_string())?;
        let reference = UnforkSnapshotReference {
            version: 2,
            operation_id: operation_id.into(),
            deployment_id: self.selection.record().deployment_id.clone(),
            receipt_digest: digest(&bytes),
        };
        let reopened = state
            .open_existing(operation_id)
            .map_err(|error| error.to_string())?;
        UnforkSnapshotReceipt::read(&reopened, &reference, limits, cancellation)?;
        self.revalidate(store, limits, cancellation)?;
        Ok(reference)
    }
}

impl PreparedSkillsShUnfork<'_> {
    pub fn publish_skills_sh_before_snapshot(
        &self,
        store: &EventStore,
        operation_id: &str,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<SkillsShUnforkSnapshotReference, String> {
        self.revalidate(store, limits, cancellation)?;
        if !valid_id(operation_id) {
            return Err("Unfork requires a new operation ID".into());
        }
        let state = BackupStateRoot::bind(&store.app_data).map_err(|e| e.to_string())?;
        let backup = state.reserve(operation_id).map_err(|e| e.to_string())?;
        let live = backup
            .copy_entry(
                &self.live.directory,
                &self.live.name,
                OsStr::new("live-tree"),
                limits,
                cancellation,
            )
            .map_err(|e| e.to_string())?;
        if live.tree_identity != self.live_identity {
            return Err("Unfork live tree changed during backup".into());
        }
        let agents = self
            .selection
            .record()
            .skill_dir
            .parent()
            .and_then(|path| path.parent())
            .ok_or("Unfork agents root missing")?;
        let root = BackupSourceRoot::bind(agents).map_err(|e| e.to_string())?;
        let mut identities = [String::new(), String::new()];
        for (index, (name, target, expected)) in [
            (".skill-lock.json", "provider-lock", &self.provider_lock),
            ("skill-studio.json", "registry-before", &self.registry),
        ]
        .into_iter()
        .enumerate()
        {
            let source = root.select(OsStr::new(name)).map_err(|e| e.to_string())?;
            let report = backup
                .copy_entry(
                    &source.directory,
                    &source.name,
                    OsStr::new(target),
                    limits,
                    cancellation,
                )
                .map_err(|e| e.to_string())?;
            if backup
                .read_record(target, 8 * 1024 * 1024)
                .map_err(|e| e.to_string())?
                != *expected
            {
                return Err("skills.sh Unfork document changed during backup".into());
            }
            identities[index] = report.tree_identity;
        }
        let receipt = SkillsShUnforkSnapshotReceipt {
            version: 1,
            operation_id: operation_id.into(),
            selection: self.selection.clone(),
            request: self.reinstall.clone(),
            live_identity: self.live_identity.clone(),
            document_identities: identities,
        };
        let bytes = serde_json::to_vec(&receipt).map_err(|e| e.to_string())?;
        if bytes.len() > MAX_RECORD {
            return Err("skills.sh Unfork snapshot receipt exceeds its limit".into());
        }
        backup
            .write_new_file("skills-sh-unfork-before.json", &bytes)
            .map_err(|e| e.to_string())?;
        let reference = SkillsShUnforkSnapshotReference {
            version: 1,
            operation_id: operation_id.into(),
            deployment_id: self.selection.record().deployment_id.clone(),
            receipt_digest: digest(&bytes),
        };
        reference.validate()?;
        self.revalidate(store, limits, cancellation)?;
        Ok(reference)
    }
}

impl SkillsShUnforkSnapshotReceipt {
    pub(super) fn read(
        backup: &ExistingBackup<'_>,
        reference: &SkillsShUnforkSnapshotReference,
        limits: BackupCopyLimits,
        cancellation: &CancellationToken,
    ) -> Result<Self, String> {
        reference.validate()?;
        if cancellation.is_cancelled() || backup.operation_id() != reference.operation_id {
            return Err("skills.sh Unfork snapshot operation changed".into());
        }
        let bytes = backup
            .read_record("skills-sh-unfork-before.json", MAX_RECORD)
            .map_err(|e| e.to_string())?;
        if digest(&bytes) != reference.receipt_digest {
            return Err("skills.sh Unfork snapshot digest changed".into());
        }
        let receipt: Self = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        receipt.selection.validate()?;
        if receipt.version != 1
            || receipt.operation_id != reference.operation_id
            || receipt.selection.record().deployment_id != reference.deployment_id
            || !receipt.selection.is_bound_current()
            || SkillsShReinstallRequest::from_fork_record(
                receipt.selection.recorded_provenance(),
                receipt.selection.name(),
                receipt.request.resolved_commit(),
            )? != receipt.request
        {
            return Err("skills.sh Unfork snapshot identities differ".into());
        }
        for (name, identity) in [
            ("live-tree", &receipt.live_identity),
            ("provider-lock", &receipt.document_identities[0]),
            ("registry-before", &receipt.document_identities[1]),
        ] {
            if !valid_digest(identity, "tree-v1:") {
                return Err("Invalid skills.sh snapshot tree identity".into());
            }
            backup
                .verify_entry(OsStr::new(name), identity, limits, cancellation)
                .map_err(|e| e.to_string())?;
        }
        let lock = backup
            .read_record("provider-lock", 8 * 1024 * 1024)
            .map_err(|e| e.to_string())?;
        let lock = crate::skill_skills_sh_fork_creation::json_document(&lock)?;
        if lock.get("version").and_then(serde_json::Value::as_u64) != Some(3)
            || lock
                .get("skills")
                .and_then(serde_json::Value::as_object)
                .is_none_or(|skills| skills.contains_key(receipt.selection.name()))
        {
            return Err("skills.sh snapshot is not detached".into());
        }
        let registry = backup
            .read_record("registry-before", 8 * 1024 * 1024)
            .map_err(|e| e.to_string())?;
        if receipt.selection.observe_document(&registry)? != UnforkRegistryState::Before {
            return Err("skills.sh snapshot owner differs".into());
        }
        if backup
            .read_record("skills-sh-unfork-before.json", MAX_RECORD)
            .map_err(|e| e.to_string())?
            != bytes
            || cancellation.is_cancelled()
        {
            return Err("skills.sh snapshot changed".into());
        }
        backup.revalidate().map_err(|e| e.to_string())?;
        Ok(receipt)
    }
}
