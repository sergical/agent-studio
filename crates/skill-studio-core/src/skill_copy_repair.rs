//! Selected-record transition data for copy repair. Hashes must come from a
//! complete scoped folder read; this value grants no filesystem write authority.
use crate::skill_fork_registry::{CopyDeploymentRecord, ForkRegistry};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyRepairTransition {
    before: CopyDeploymentRecord,
    after: CopyDeploymentRecord,
}

impl CopyRepairTransition {
    pub fn new(before: CopyDeploymentRecord, repaired_folder_hash: String) -> Result<Self, String> {
        let mut after = before.clone();
        after.content_hash = repaired_folder_hash;
        let transition = Self { before, after };
        transition.validate()?;
        Ok(transition)
    }

    pub(crate) fn is_disabled(&self) -> bool {
        self.before.disabled
    }

    pub fn validate(&self) -> Result<(), String> {
        use crate::skill_deployment::{deployment_id, InstallScope};
        let before = &self.before;
        let scope = match before.scope {
            InstallScope::Global => "global",
            InstallScope::Project => "project",
        };
        let identity = deployment_id(
            &before.name,
            scope,
            before.destination,
            &before.slot,
            before.project_path.as_deref(),
            &before.path,
        );
        let hash_valid = |hash: &str| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        };
        let mut expected = before.clone();
        expected.content_hash.clone_from(&self.after.content_hash);
        if before.deployment_id != identity
            || !before.path.is_absolute()
            || before.name.is_empty()
            || before.name.contains(['/', '\\'])
            || matches!(before.name.as_str(), "." | "..")
            || before
                .path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            || !hash_valid(&before.content_hash)
            || !hash_valid(&self.after.content_hash)
            || before.content_hash == self.after.content_hash
            || self.after != expected
        {
            return Err("Invalid copy repair registry transition".into());
        }
        Ok(())
    }

    pub(crate) fn before(&self) -> &CopyDeploymentRecord {
        &self.before
    }

    #[cfg(all(unix, feature = "event-store"))]
    pub(crate) fn after(&self) -> &CopyDeploymentRecord {
        &self.after
    }

    pub fn apply(&self, registry: &mut ForkRegistry) -> Result<bool, String> {
        self.replace(registry, &self.before, &self.after)
    }

    pub fn roll_back(&self, registry: &mut ForkRegistry) -> Result<bool, String> {
        self.replace(registry, &self.after, &self.before)
    }

    pub fn apply_document(&self, original: &[u8]) -> Result<Vec<u8>, String> {
        self.replace_document(original, &self.before, &self.after)
    }

    pub fn roll_back_document(&self, original: &[u8]) -> Result<Vec<u8>, String> {
        self.replace_document(original, &self.after, &self.before)
    }

    fn replace_document(
        &self,
        original: &[u8],
        expected: &CopyDeploymentRecord,
        replacement: &CopyDeploymentRecord,
    ) -> Result<Vec<u8>, String> {
        self.validate()?;
        const LIMIT: usize = 8 * 1024 * 1024;
        if original.len() > LIMIT {
            return Err("Copy registry exceeds its limit".into());
        }
        let mut document: serde_json::Value =
            serde_json::from_slice(original).map_err(|error| error.to_string())?;
        let entry = document
            .get_mut("copies")
            .and_then(|copies| copies.get_mut(&self.before.deployment_id))
            .ok_or("Copy repair registry record is missing")?;
        let current: CopyDeploymentRecord =
            serde_json::from_value(entry.clone()).map_err(|error| error.to_string())?;
        if current == *replacement {
            return Ok(original.to_vec());
        }
        if current != *expected {
            return Err("Copy repair registry record changed".into());
        }
        entry
            .as_object_mut()
            .ok_or("Copy registry record is not an object")?
            .insert(
                "content_hash".into(),
                serde_json::Value::String(replacement.content_hash.clone()),
            );
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
        if bytes.len() > LIMIT {
            return Err("Copy registry exceeds its limit".into());
        }
        Ok(bytes)
    }

    fn replace(
        &self,
        registry: &mut ForkRegistry,
        expected: &CopyDeploymentRecord,
        replacement: &CopyDeploymentRecord,
    ) -> Result<bool, String> {
        self.validate()?;
        let current = registry
            .copies
            .get_mut(&self.before.deployment_id)
            .ok_or("Copy repair registry record is missing")?;
        if current == replacement {
            return Ok(false);
        }
        if current != expected {
            return Err("Copy repair registry record changed".into());
        }
        *current = replacement.clone();
        Ok(true)
    }
}

/// Saved consistency evidence for the two-file operation. Recorded paths must
/// be rebound to an authorized scope before any recovery reads or writes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyRepairIntent {
    pub document: crate::skill_repair_intent::FrontmatterRepairIntent,
    pub transition: CopyRepairTransition,
    pub registry_path: std::path::PathBuf,
    pub registry_before_fingerprint: String,
    pub registry_after_fingerprint: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyRepairObservedState {
    Original,
    DocumentApplied,
    Applied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyUndoObservedState {
    Unchanged,
    DocumentRestored,
    Restored,
}

impl CopyRepairIntent {
    pub fn from_prepared(
        prepared: &crate::skill_service::PreparedCopyRepairSelection<'_>,
    ) -> Result<Self, String> {
        use crate::skill_frontmatter_repair::{content_fingerprint, FrontmatterRepairApplyMode};
        prepared.revalidate()?;
        let (path, before, after) = prepared.registry_change();
        let intent = Self {
            document: crate::skill_repair_intent::FrontmatterRepairIntent::from_preview(
                prepared.preview(),
                FrontmatterRepairApplyMode::ApplyFix,
                None,
            )?,
            transition: prepared.transition().clone(),
            registry_path: path.into(),
            registry_before_fingerprint: content_fingerprint(before),
            registry_after_fingerprint: content_fingerprint(after),
        };
        intent.validate_record()?;
        intent.validate_registry_original(before)?;
        Ok(intent)
    }

    pub fn validate_record(&self) -> Result<(), String> {
        use crate::{
            skill_fork_registry::RegistryOwnerRecord,
            skill_frontmatter_repair::{proposal_id, FrontmatterRepairApplyMode},
            skill_inventory::Deployment,
            skill_ownership::LifecycleOwnerKind,
        };
        self.document.validate_record()?;
        self.transition.validate()?;
        let record = &self.transition.before;
        let deployment = Deployment {
            id: record.deployment_id.clone(),
            path: record.path.to_string_lossy().into_owned(),
            owner_kind: LifecycleOwnerKind::Copy,
            owner_revision: RegistryOwnerRecord::Copy(record).revision(),
            ..Deployment::default()
        };
        let proposal = proposal_id(
            &deployment,
            &self.document.expected_content_fingerprint,
            &self.document.proposed_content,
        );
        let valid_hash = |value: &str| {
            value.strip_prefix("sha256:").is_some_and(|hash| {
                hash.len() == 64
                    && hash
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
        };
        if self.document.mode != FrontmatterRepairApplyMode::ApplyFix
            || self.document.deployment_id != record.deployment_id
            || self.document.path != record.path
            || self.document.name != record.name
            || self.document.proposal_id.as_deref() != Some(proposal.as_str())
            || !self.registry_path.is_absolute()
            || self.registry_path.file_name() != Some(std::ffi::OsStr::new("skill-studio.json"))
            || self
                .registry_path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            || !valid_hash(&self.registry_before_fingerprint)
            || !valid_hash(&self.registry_after_fingerprint)
            || self.registry_before_fingerprint == self.registry_after_fingerprint
        {
            return Err(
                "Copy repair intent does not match its document and registry transition".into(),
            );
        }
        Ok(())
    }

    /// Classifies independently read evidence. The caller must bind the paths,
    /// verify original backups and retain the read lease before acting on this.
    pub fn classify_observed(
        &self,
        document: &[u8],
        registry: &[u8],
        folder_hash: &str,
    ) -> Result<CopyRepairObservedState, String> {
        let (document_applied, registry_applied) =
            self.observed_sides(document, registry, folder_hash)?;
        match (document_applied, registry_applied) {
            (false, false) => Ok(CopyRepairObservedState::Original),
            (true, false) => Ok(CopyRepairObservedState::DocumentApplied),
            (true, true) => Ok(CopyRepairObservedState::Applied),
            (false, true) => Err("Copy registry was updated without its repaired document".into()),
        }
    }
    pub fn classify_undo_observed(
        &self,
        document: &[u8],
        registry: &[u8],
        folder_hash: &str,
    ) -> Result<CopyUndoObservedState, String> {
        match self.observed_sides(document, registry, folder_hash)? {
            (true, true) => Ok(CopyUndoObservedState::Unchanged),
            (false, true) => Ok(CopyUndoObservedState::DocumentRestored),
            (false, false) => Ok(CopyUndoObservedState::Restored),
            (true, false) => Err("Copy undo registry changed before its document".into()),
        }
    }

    fn observed_sides(
        &self,
        document: &[u8],
        registry: &[u8],
        folder_hash: &str,
    ) -> Result<(bool, bool), String> {
        use crate::skill_frontmatter_repair::content_fingerprint;
        self.validate_record()?;
        if document.len() > crate::skill_service::MAX_REPAIR_DOCUMENT_BYTES
            || registry.len() > 8 * 1024 * 1024
        {
            return Err("Copy repair observation exceeds its limit".into());
        }
        let fingerprint = content_fingerprint(document);
        let document_applied = if fingerprint == self.document.expected_content_fingerprint {
            self.document.validate_original(document)?;
            false
        } else if fingerprint == self.document.proposed_content_fingerprint {
            true
        } else {
            return Err("Copy repair document conflicts with its intent".into());
        };
        let expected_hash = if document_applied {
            &self.transition.after.content_hash
        } else {
            &self.transition.before.content_hash
        };
        if folder_hash != expected_hash {
            return Err("Copy repair resources conflict with its intent".into());
        }
        let registry: serde_json::Value =
            serde_json::from_slice(registry).map_err(|error| error.to_string())?;
        let value = registry
            .get("copies")
            .and_then(|copies| copies.get(&self.document.deployment_id))
            .ok_or("Copy repair registry record is missing")?;
        let current: CopyDeploymentRecord =
            serde_json::from_value(value.clone()).map_err(|error| error.to_string())?;
        let registry_applied = if current == self.transition.before {
            false
        } else if current == self.transition.after {
            true
        } else {
            return Err("Copy repair registry record conflicts with its intent".into());
        };
        Ok((document_applied, registry_applied))
    }

    #[cfg(all(unix, feature = "event-store"))]
    pub(crate) fn validate_repaired_backup(
        &self,
        document: &[u8],
        registry: &[u8],
    ) -> Result<(), String> {
        if self.classify_observed(document, registry, &self.transition.after.content_hash)?
            != CopyRepairObservedState::Applied
        {
            return Err("Undo backup is not the repaired document and copy record".into());
        }
        Ok(())
    }

    pub fn validate_registry_original(&self, bytes: &[u8]) -> Result<(), String> {
        use crate::skill_frontmatter_repair::content_fingerprint;
        self.validate_record()?;
        if content_fingerprint(bytes) != self.registry_before_fingerprint {
            return Err("Copy repair original registry does not match its intent".into());
        }
        let proposed = self.transition.apply_document(bytes)?;
        if content_fingerprint(&proposed) != self.registry_after_fingerprint {
            return Err("Copy repair proposed registry does not match its intent".into());
        }
        Ok(())
    }
}

/// Byte plan only. Callers must obtain and retain live-scope authorization and
/// preserve these expected bytes before claiming or publishing an undo.
#[cfg(all(unix, feature = "event-store"))]
pub struct CopyRepairUndoPlan {
    document_expected: Vec<u8>,
    document_restored: Vec<u8>,
    registry_expected: Vec<u8>,
    registry_restored: Vec<u8>,
}

#[cfg(all(unix, feature = "event-store"))]
impl CopyRepairUndoPlan {
    pub fn from_observed(
        source: &crate::skill_repair_recovery_event::CopyRepairUndoSource,
        backup: &crate::skill_repair_backup::VerifiedCopyRepairBackup,
        document: &[u8],
        registry: &[u8],
        folder_hash: &str,
    ) -> Result<Self, String> {
        let intent = source.intent();
        if intent.classify_observed(document, registry, folder_hash)?
            != CopyRepairObservedState::Applied
        {
            return Err("Copy undo requires the complete repaired state".into());
        }
        let (original_document, original_registry) = backup.originals();
        intent.document.validate_original(original_document)?;
        intent.validate_registry_original(original_registry)?;
        let registry_restored = intent.transition.roll_back_document(registry)?;
        Ok(Self {
            document_expected: document.into(),
            document_restored: original_document.into(),
            registry_expected: registry.into(),
            registry_restored,
        })
    }

    pub fn document_change(&self) -> (&[u8], &[u8]) {
        (&self.document_expected, &self.document_restored)
    }
    pub fn registry_change(&self) -> (&[u8], &[u8]) {
        (&self.registry_expected, &self.registry_restored)
    }
}

/// Supplied-evidence byte plan; the caller must separately retain and validate a live scope.
#[cfg(all(unix, feature = "event-store"))]
pub struct CopyRepairRedoPlan {
    document_expected: Vec<u8>,
    document_repaired: Vec<u8>,
    registry_expected: Vec<u8>,
    registry_repaired: Vec<u8>,
}

#[cfg(all(unix, feature = "event-store"))]
impl CopyRepairRedoPlan {
    pub fn from_observed(
        source: &crate::skill_repair_recovery_event::CopyRepairRedoSource,
        backups: &crate::skill_repair_backup::VerifiedCopyUndoBackups,
        document: &[u8],
        registry: &[u8],
        folder_hash: &str,
    ) -> Result<Self, String> {
        let intent = source.intent();
        if intent.classify_undo_observed(document, registry, folder_hash)?
            != CopyUndoObservedState::Restored
        {
            return Err("Copy redo requires the complete undone state".into());
        }
        let (original_document, original_registry) = backups.repair_originals();
        intent.document.validate_original(original_document)?;
        intent.validate_registry_original(original_registry)?;
        let (repaired_document, repaired_registry) = backups.undo_originals();
        intent.validate_repaired_backup(repaired_document, repaired_registry)?;
        Ok(Self {
            document_expected: document.into(),
            document_repaired: repaired_document.into(),
            registry_expected: registry.into(),
            registry_repaired: intent.transition.apply_document(registry)?,
        })
    }
    pub fn document_change(&self) -> (&[u8], &[u8]) {
        (&self.document_expected, &self.document_repaired)
    }
    pub fn registry_change(&self) -> (&[u8], &[u8]) {
        (&self.registry_expected, &self.registry_repaired)
    }
}

#[cfg(all(unix, feature = "event-store"))]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CopyRepairRedoIntent {
    pub source_event: String,
    pub undo_event: String,
    pub repair: CopyRepairIntent,
}

#[cfg(all(unix, feature = "event-store"))]
impl CopyRepairRedoIntent {
    pub fn from_prepared(
        prepared: &crate::skill_service::PreparedCopyRepairRedo<'_>,
    ) -> Result<Self, String> {
        use crate::skill_frontmatter_repair::content_fingerprint;
        prepared.revalidate()?;
        let mut repair = prepared.source.intent().clone();
        let (before, after) = prepared.plan.registry_change();
        repair.registry_before_fingerprint = content_fingerprint(before);
        repair.registry_after_fingerprint = content_fingerprint(after);
        repair.validate_registry_original(before)?;
        let intent = Self {
            source_event: prepared.source.source().id.clone(),
            undo_event: prepared.source.undo().id.clone(),
            repair,
        };
        intent.validate_source(&prepared.source)?;
        Ok(intent)
    }

    pub fn validate_record(&self) -> Result<(), String> {
        if !crate::skill_backup_reservation::valid_id(&self.source_event)
            || !crate::skill_backup_reservation::valid_id(&self.undo_event)
            || self.source_event == self.undo_event
        {
            return Err("Invalid copy redo history IDs".into());
        }
        self.repair.validate_record()
    }

    pub fn validate_source(
        &self,
        source: &crate::skill_repair_recovery_event::CopyRepairRedoSource,
    ) -> Result<(), String> {
        self.validate_record()?;
        let mut expected = source.intent().clone();
        expected
            .registry_before_fingerprint
            .clone_from(&self.repair.registry_before_fingerprint);
        expected
            .registry_after_fingerprint
            .clone_from(&self.repair.registry_after_fingerprint);
        if self.source_event != source.source().id
            || self.undo_event != source.undo().id
            || serde_json::to_value(&expected).map_err(|error| error.to_string())?
                != serde_json::to_value(&self.repair).map_err(|error| error.to_string())?
        {
            return Err("Copy redo intent differs from its completed undo source".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_deployment::{deployment_id, InstallScope, SkillDestination};

    fn record() -> CopyDeploymentRecord {
        let path = std::path::PathBuf::from("/fixture/.codex/skills/alpha");
        CopyDeploymentRecord {
            deployment_id: deployment_id(
                "alpha",
                "global",
                SkillDestination::PerHarness,
                "codex",
                None,
                &path,
            ),
            name: "alpha".into(),
            path,
            scope: InstallScope::Global,
            destination: SkillDestination::PerHarness,
            slot: "codex".into(),
            project_path: None,
            content_hash: "a".repeat(64),
            disabled: false,
        }
    }

    #[test]
    fn applies_and_rolls_back_only_the_exact_copy_idempotently() {
        let before = record();
        let transition = CopyRepairTransition::new(before.clone(), "b".repeat(64)).unwrap();
        let mut registry = ForkRegistry::default();
        registry
            .copies
            .insert(before.deployment_id.clone(), before.clone());
        registry.copies.insert("unrelated".into(), before);
        let original = serde_json::to_value(&registry).unwrap();
        assert!(transition.apply(&mut registry).unwrap());
        assert!(!transition.apply(&mut registry).unwrap());
        assert_eq!(registry.copies["unrelated"].content_hash, "a".repeat(64));
        assert!(transition.roll_back(&mut registry).unwrap());
        assert!(!transition.roll_back(&mut registry).unwrap());
        assert_eq!(serde_json::to_value(&registry).unwrap(), original);
    }

    #[test]
    fn document_transition_preserves_unknown_fields_and_noop_bytes() {
        let before = record();
        let transition = CopyRepairTransition::new(before.clone(), "b".repeat(64)).unwrap();
        let id = before.deployment_id.clone();
        let mut value =
            serde_json::json!({"future_setting": {"nested": [1, true, "value"]}, "copies": {}});
        value["copies"][&id] = serde_json::to_value(before).unwrap();
        value["copies"][&id]["future_record_field"] = serde_json::json!({"keep": true});
        value["copies"]["unrelated"] = serde_json::json!({"future": "shape"});
        let original = serde_json::to_vec(&value).unwrap();
        let applied = transition.apply_document(&original).unwrap();
        let mut expected = value.clone();
        expected["copies"][&id]["content_hash"] = serde_json::json!("b".repeat(64));
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&applied).unwrap(),
            expected
        );
        assert_eq!(transition.apply_document(&applied).unwrap(), applied);
        let restored = transition.roll_back_document(&applied).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&restored).unwrap(),
            value
        );
        assert_eq!(transition.roll_back_document(&original).unwrap(), original);
        value["copies"][&id]["disabled"] = serde_json::json!(true);
        let changed = serde_json::to_vec(&value).unwrap();
        assert!(transition.apply_document(&changed).is_err());
        assert!(transition.roll_back_document(&changed).is_err());
    }

    #[test]
    fn refuses_drift_and_malformed_saved_transitions() {
        let before = record();
        let transition = CopyRepairTransition::new(before.clone(), "b".repeat(64)).unwrap();
        let mut registry = ForkRegistry::default();
        assert!(transition.apply(&mut registry).is_err());
        let mut changed = before.clone();
        changed.disabled = true;
        registry
            .copies
            .insert(before.deployment_id.clone(), changed);
        let original = serde_json::to_value(&registry).unwrap();
        assert!(transition.apply(&mut registry).is_err());
        assert!(transition.roll_back(&mut registry).is_err());
        assert_eq!(serde_json::to_value(&registry).unwrap(), original);
        for hash in ["", "xyz", &"A".repeat(64), &"a".repeat(64)] {
            assert!(CopyRepairTransition::new(before.clone(), hash.into()).is_err());
        }
        let mut value = serde_json::to_value(&transition).unwrap();
        value["after"]["path"] = serde_json::json!("/elsewhere");
        let malformed: CopyRepairTransition = serde_json::from_value(value).unwrap();
        assert!(malformed.apply(&mut registry).is_err());
        assert_eq!(serde_json::to_value(&registry).unwrap(), original);
    }
}
