//! Consistent dotagents fork data; live scope, provenance and event binding remain mandatory.
use crate::{
    skill_fork_registry::OriginTool, skill_fork_snapshot::ForkSnapshotReference,
    skill_fork_transition::ForkRegistryTransition,
    skill_frontmatter_repair::FrontmatterRepairApplyMode,
    skill_repair_intent::FrontmatterRepairIntent,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DotagentsForkRepairIntent {
    version: u32,
    repair: FrontmatterRepairIntent,
    registry: ForkRegistryTransition,
    snapshots: ForkSnapshotReference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    provider_documents: Option<crate::skill_dotagents_ledger::DotagentsDetachProposal>,
}

impl DotagentsForkRepairIntent {
    pub fn new(
        repair: FrontmatterRepairIntent,
        registry: ForkRegistryTransition,
        snapshots: ForkSnapshotReference,
    ) -> Result<Self, String> {
        let intent = Self {
            version: 1,
            repair,
            registry,
            snapshots,
            provider_documents: None,
        };
        intent.validate_for_operation(intent.snapshots.operation_id())?;
        Ok(intent)
    }

    pub(crate) fn from_prepared(
        prepared: &crate::skill_service::PreparedDotagentsForkSelection<'_>,
        snapshots: ForkSnapshotReference,
        forked_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<Self, String> {
        prepared.revalidate().map_err(|error| error.to_string())?;
        let source = prepared.fork_source()?;
        let bytes = prepared.registry_before().unwrap_or(b"{}");
        let before = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        let repair = FrontmatterRepairIntent::from_preview(
            prepared.preview(),
            FrontmatterRepairApplyMode::ForkAndFix,
            Some(before),
        )?;
        let record = crate::skill_fork_registry::ForkRecord {
            deployment_id: repair.deployment_id.clone(),
            skill_dir: repair.path.clone(),
            forked_at: forked_at.to_rfc3339(),
            origin_tool: OriginTool::Dotagents,
            origin_source: source.source().into(),
            repo: source.repo().into(),
            path: source.path().into(),
            declared_ref: source.declared_ref().map(str::to_owned),
            base_commit: source.commit().into(),
        };
        let registry = ForkRegistryTransition::new(repair.name.clone(), record, bytes)?;
        let proposal = prepared.detach().propose_document_detach(
            std::str::from_utf8(prepared.provider_lock()).map_err(|error| error.to_string())?,
            std::str::from_utf8(prepared.provider_manifest()).map_err(|error| error.to_string())?,
        )?;
        let mut intent = Self::new(repair, registry, snapshots)?;
        intent.version = 2;
        intent.provider_documents = Some(proposal);
        intent.validate_for_operation(intent.snapshots.operation_id())?;
        prepared.revalidate().map_err(|error| error.to_string())?;
        Ok(intent)
    }

    /// Compare with a receipt already opened against this intent's snapshot reference.
    pub fn validate_snapshot_source(
        &self,
        receipt: &crate::skill_fork_snapshot::ForkSnapshotReceipt,
    ) -> Result<(), String> {
        self.validate_for_operation(self.snapshots.operation_id())?;
        let detach = receipt.detach()?;
        let source = detach.fork_source()?;
        let record = self.registry.record();
        if detach.name() != self.repair.name
            || record.origin_source != source.source()
            || record.repo != source.repo()
            || record.path != source.path()
            || record.base_commit != source.commit()
            || record.declared_ref.as_deref() != source.declared_ref()
        {
            return Err("Fork registry source differs from saved provider ownership".into());
        }
        Ok(())
    }

    pub fn provider_documents(
        &self,
    ) -> Option<&crate::skill_dotagents_ledger::DotagentsDetachProposal> {
        self.provider_documents.as_ref()
    }

    pub fn validate_saved_provider_documents(
        &self,
        lock: &str,
        manifest: &str,
    ) -> Result<(), String> {
        self.validate_for_operation(self.snapshots.operation_id())?;
        if let Some(proposal) = &self.provider_documents {
            let detach = crate::skill_dotagents_ledger::DotagentsDetachIntent::from_documents(
                &self.repair.name,
                lock,
                manifest,
            )?;
            if &detach.propose_document_detach(lock, manifest)? != proposal {
                return Err(
                    "Saved provider inputs do not produce the exact pending proposal".into(),
                );
            }
        }
        Ok(())
    }

    pub fn repair(&self) -> &FrontmatterRepairIntent {
        &self.repair
    }
    pub fn registry(&self) -> &ForkRegistryTransition {
        &self.registry
    }
    pub fn snapshots(&self) -> &ForkSnapshotReference {
        &self.snapshots
    }

    pub fn validate_for_operation(&self, operation_id: &str) -> Result<(), String> {
        self.repair.validate_record()?;
        self.registry.validate()?;
        self.snapshots.validate()?;
        let record = self.registry.record();
        if self.repair.proposed_content.len() > crate::skill_service::MAX_REPAIR_DOCUMENT_BYTES
            || !matches!(
                (self.version, self.provider_documents.is_some()),
                (1, false) | (2, true)
            )
            || self.repair.mode != FrontmatterRepairApplyMode::ForkAndFix
            || self
                .repair
                .proposal_id
                .as_ref()
                .is_none_or(|id| id.is_empty())
            || self.repair.managed_update_warning
            || record.origin_tool != OriginTool::Dotagents
            || record.deployment_id != self.repair.deployment_id
            || record.skill_dir != self.repair.path
            || self.snapshots.deployment_id() != self.repair.deployment_id
            || self.snapshots.operation_id() != operation_id
        {
            return Err("Fork repair parts do not identify one operation and deployment".into());
        }
        if let Some(proposal) = &self.provider_documents {
            proposal.validate(&self.repair.name)?;
        }
        let before = self
            .repair
            .fork_registry_before
            .as_ref()
            .ok_or("Fork repair has no registry before-state")?;
        if before.forks.contains_key(&self.repair.name) {
            return Err("Fork repair before-state already contains the selected fork".into());
        }
        self.registry.validate_before_projection(before)?;
        Ok(())
    }
}

#[cfg(feature = "event-store")]
#[derive(Debug)]
pub struct DotagentsForkRecoveryEvent {
    snapshot: crate::skill_event::EventRow,
    intent: DotagentsForkRepairIntent,
}

#[cfg(feature = "event-store")]
impl DotagentsForkRecoveryEvent {
    pub fn from_row(row: &crate::skill_event::EventRow) -> Result<Self, String> {
        if !matches!(row.status.as_str(), "pending" | "interrupted") {
            return Err("Invalid dotagents fork recovery row".into());
        }
        let intent = read_unclaimed_fork_intent(row)?;
        Ok(Self {
            snapshot: row.clone(),
            intent,
        })
    }

    pub fn id(&self) -> &str {
        &self.snapshot.id
    }
    pub fn intent(&self) -> &DotagentsForkRepairIntent {
        &self.intent
    }
    pub(crate) fn snapshot(&self) -> &crate::skill_event::EventRow {
        &self.snapshot
    }
}

/// Completed source evidence only; current ownership and saved files still need scoped validation.
#[cfg(feature = "event-store")]
#[derive(Debug)]
pub struct CompletedDotagentsForkEvent {
    snapshot: crate::skill_event::EventRow,
    intent: DotagentsForkRepairIntent,
}

#[cfg(feature = "event-store")]
impl CompletedDotagentsForkEvent {
    pub fn from_row(row: &crate::skill_event::EventRow) -> Result<Self, String> {
        if row.status != "done" {
            return Err("Fork document restore requires a completed fork event".into());
        }
        Ok(Self {
            snapshot: row.clone(),
            intent: read_unclaimed_fork_intent(row)?,
        })
    }

    pub(crate) fn from_claimed_row(
        row: &crate::skill_event::EventRow,
        claim: &str,
    ) -> Result<Self, String> {
        if row.status != "done"
            || !crate::skill_backup_reservation::valid_id(claim)
            || row.id == claim
        {
            return Err("Fork restore recovery requires a completed claimed source".into());
        }
        Ok(Self {
            snapshot: row.clone(),
            intent: read_fork_intent(row, Some(claim))?,
        })
    }

    pub fn event(&self) -> &crate::skill_event::EventRow {
        &self.snapshot
    }

    pub fn intent(&self) -> &DotagentsForkRepairIntent {
        &self.intent
    }
}

#[cfg(feature = "event-store")]
fn read_unclaimed_fork_intent(
    row: &crate::skill_event::EventRow,
) -> Result<DotagentsForkRepairIntent, String> {
    read_fork_intent(row, None)
}

#[cfg(feature = "event-store")]
fn read_fork_intent(
    row: &crate::skill_event::EventRow,
    claim: Option<&str>,
) -> Result<DotagentsForkRepairIntent, String> {
    if row.kind != "repair_dotagents_fork"
        || row.restorable
        || row.inverse.is_some()
        || row.reverted_by.as_deref() != claim
        || row.harness.is_some()
        || row.project_path.is_some()
        || row.scope.as_deref() != Some("global")
        || row.backup_dir.as_deref() != Some(format!("backups/{}", row.id).as_str())
    {
        return Err("Invalid dotagents fork row or claim".into());
    }
    let intent: DotagentsForkRepairIntent =
        serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?;
    intent.validate_for_operation(&row.id)?;
    if row.skill != intent.repair().name
        || serde_json::to_value(&intent).map_err(|error| error.to_string())? != row.payload
    {
        return Err("Fork event target or payload does not match its intent".into());
    }
    Ok(intent)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::{
        skill_deployment::{deployment_id, SkillDestination},
        skill_fork_registry::{ForkRecord, ForkRegistry},
        skill_frontmatter_repair::{content_fingerprint, propose_colon_scalar_repair},
    };
    use std::path::PathBuf;

    pub(crate) fn fixture() -> DotagentsForkRepairIntent {
        let path = PathBuf::from("/fixture/.agents/skills/alpha");
        let id = deployment_id(
            "alpha",
            "global",
            SkillDestination::Universal,
            "universal",
            None,
            &path,
        );
        let original = "---\nname: alpha\ndescription: Use when: testing\n---\nbody\n";
        let (proposed, _) = propose_colon_scalar_repair(original).unwrap();
        let registry = ForkRegistry::default();
        let repair = FrontmatterRepairIntent {
            deployment_id: id.clone(),
            proposal_id: Some("bound-proposal".into()),
            name: "alpha".into(),
            path: path.clone(),
            expected_content_fingerprint: content_fingerprint(original.as_bytes()),
            proposed_content_fingerprint: content_fingerprint(proposed.as_bytes()),
            proposed_content: proposed,
            mode: FrontmatterRepairApplyMode::ForkAndFix,
            managed_update_warning: false,
            fork_registry_before: Some(registry.clone()),
        };
        let record = ForkRecord {
            deployment_id: id.clone(),
            skill_dir: path,
            forked_at: "2026-09-11T00:00:00Z".into(),
            origin_tool: OriginTool::Dotagents,
            origin_source: "owner/repo".into(),
            repo: "owner/repo".into(),
            path: "skills/alpha".into(),
            declared_ref: None,
            base_commit: "a".repeat(40),
        };
        let transition = ForkRegistryTransition::new(
            "alpha".into(),
            record,
            &serde_json::to_vec(&registry).unwrap(),
        )
        .unwrap();
        let snapshots = serde_json::from_value(serde_json::json!({"version":1,"operation_id":"fork","deployment_id":id,"receipt_digest":format!("sha256:{}", "b".repeat(64))})).unwrap();
        DotagentsForkRepairIntent::new(repair, transition, snapshots).unwrap()
    }

    #[test]
    fn native_intent_requires_versioned_exact_provider_documents() {
        let lock = "version = 1\n[skills.alpha]\nsource = 'owner/repo'\n";
        let manifest = "version = 1\n[[skills]]\nname = 'alpha'\nsource = 'owner/repo'\n";
        let proposal = crate::skill_dotagents_ledger::DotagentsDetachIntent::from_documents(
            "alpha", lock, manifest,
        )
        .unwrap()
        .propose_document_detach(lock, manifest)
        .unwrap();
        let mut value = serde_json::to_value(fixture()).unwrap();
        value["version"] = serde_json::json!(2);
        value["provider_documents"] = serde_json::to_value(proposal).unwrap();
        let intent: DotagentsForkRepairIntent = serde_json::from_value(value.clone()).unwrap();
        intent.validate_for_operation("fork").unwrap();
        intent
            .validate_saved_provider_documents(lock, manifest)
            .unwrap();
        for version in [1, 3] {
            let mut changed = value.clone();
            changed["version"] = serde_json::json!(version);
            assert!(serde_json::from_value::<DotagentsForkRepairIntent>(changed)
                .unwrap()
                .validate_for_operation("fork")
                .is_err());
        }
        let mut missing = value.clone();
        missing
            .as_object_mut()
            .unwrap()
            .remove("provider_documents");
        assert!(serde_json::from_value::<DotagentsForkRepairIntent>(missing)
            .unwrap()
            .validate_for_operation("fork")
            .is_err());
        let mut attached = value.clone();
        attached["provider_documents"]["lock"] = serde_json::json!(lock);
        assert!(
            serde_json::from_value::<DotagentsForkRepairIntent>(attached)
                .unwrap()
                .validate_for_operation("fork")
                .is_err()
        );
        value["provider_documents"]["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<DotagentsForkRepairIntent>(value).is_err());
    }

    #[cfg(feature = "event-store")]
    #[test]
    fn completed_fork_source_rejects_unfinished_claimed_and_mismatched_history() {
        let intent = fixture();
        let row = crate::skill_event::EventRow {
            id: "fork".into(),
            ts: "2026-09-13T00:00:00Z".into(),
            kind: "repair_dotagents_fork".into(),
            skill: "alpha".into(),
            harness: None,
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::to_value(&intent).unwrap(),
            inverse: None,
            backup_dir: Some("backups/fork".into()),
            status: "done".into(),
            reverted_by: None,
            restorable: false,
        };
        let source = CompletedDotagentsForkEvent::from_row(&row).unwrap();
        assert_eq!(source.event().id, "fork");
        assert_eq!(
            source.intent().repair().deployment_id,
            intent.repair().deployment_id
        );
        assert_eq!(source.intent().snapshots().operation_id(), "fork");
        assert!(DotagentsForkRecoveryEvent::from_row(&row).is_err());
        for status in ["pending", "interrupted", "failed"] {
            let mut changed = row.clone();
            changed.status = status.into();
            assert!(CompletedDotagentsForkEvent::from_row(&changed).is_err());
            assert_eq!(
                DotagentsForkRecoveryEvent::from_row(&changed).is_ok(),
                status != "failed"
            );
        }
        for (field, value) in [
            ("id", serde_json::json!("other")),
            ("kind", serde_json::json!("repair_skill_frontmatter")),
            ("skill", serde_json::json!("other")),
            ("scope", serde_json::json!("project")),
            ("project_path", serde_json::json!("/other")),
            ("harness", serde_json::json!("codex")),
            ("backup_dir", serde_json::json!("backups/other")),
            ("reverted_by", serde_json::json!("already-restored")),
            ("restorable", serde_json::json!(true)),
            ("inverse", serde_json::json!({})),
        ] {
            let mut changed = serde_json::to_value(&row).unwrap();
            changed[field] = value;
            let changed = serde_json::from_value(changed).unwrap();
            assert!(
                CompletedDotagentsForkEvent::from_row(&changed).is_err(),
                "{field}"
            );
        }
        let mut changed = row;
        changed.payload["snapshots"]["operation_id"] = serde_json::json!("other");
        assert!(CompletedDotagentsForkEvent::from_row(&changed).is_err());
    }

    #[test]
    fn intent_rejects_cross_deployment_parts_and_inconsistent_before_state() {
        let original = serde_json::to_value(fixture()).unwrap();
        let restored: DotagentsForkRepairIntent = serde_json::from_value(original.clone()).unwrap();
        restored.validate_for_operation("fork").unwrap();
        assert!(restored.validate_for_operation("other").is_err());
        for (pointer, value) in [
            ("/version", serde_json::json!(2)),
            ("/repair/proposal_id", serde_json::Value::Null),
            ("/repair/managed_update_warning", serde_json::json!(true)),
            (
                "/registry/record/skill_dir",
                serde_json::json!("/other/.agents/skills/alpha"),
            ),
            ("/snapshots/deployment_id", serde_json::json!("other")),
            ("/snapshots/operation_id", serde_json::json!("other")),
            (
                "/registry/trials_before",
                serde_json::json!({"global/alpha":{"unexpected":true}}),
            ),
        ] {
            let mut changed = original.clone();
            *changed.pointer_mut(pointer).unwrap() = value;
            let intent: DotagentsForkRepairIntent = serde_json::from_value(changed).unwrap();
            assert!(intent.validate_for_operation("fork").is_err(), "{pointer}");
        }
    }
}
