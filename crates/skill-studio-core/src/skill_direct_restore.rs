//! Scoped document restore preserves the overwritten bytes and linked history.
use crate::{
    skill_backup_reservation::{BackupCopyLimits, BackupStateRoot},
    skill_backup_source::BackupSourceRoot,
    skill_coordination::FinalizedWriteLease,
    skill_document_target::SkillDocumentTarget,
    skill_event::{EventDraft, EventRow, EventStatus, InverseOp},
    skill_event_store::fingerprint_regular_bytes,
    skill_event_worker_protocol::PreparedEventExchange,
    skill_repair_backup::VerifiedRepairBackup,
    skill_repair_execution::{
        RepairEvents, RepairExecutionError, RepairExecutionReceipt, RepairExecutionStage,
    },
    skill_repair_intent::FrontmatterRepairIntent,
    skill_repair_worker::RepairEventWorker,
};
use std::{ffi::OsStr, path::Path};

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DirectRestoreIntent {
    pub target_event: String,
    pub repair: FrontmatterRepairIntent,
    pub before: String,
    pub after: String,
}

pub(crate) struct DirectRestoreSource {
    pub row: EventRow,
    pub repair: FrontmatterRepairIntent,
    pub before: String,
    pub after: String,
}
impl DirectRestoreSource {
    pub(crate) fn from_row(row: &EventRow, claim: Option<&str>) -> Result<Self, String> {
        if row.status != "done"
            || !row.restorable
            || row.reverted_by.as_deref() != claim
            || !crate::skill_backup_reservation::valid_id(&row.id)
            || row.backup_dir.as_deref() != Some(format!("backups/{}", row.id).as_str())
        {
            return Err("Restore requires a completed event with the expected claim".into());
        }
        let repair: FrontmatterRepairIntent = match row.kind.as_str() {
            "repair_skill_frontmatter" => {
                serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?
            }
            "restore" => {
                let intent: DirectRestoreIntent = serde_json::from_value(row.payload.clone())
                    .map_err(|error| error.to_string())?;
                if intent.target_event == row.id
                    || !crate::skill_backup_reservation::valid_id(&intent.target_event)
                {
                    return Err("Invalid restore source link".into());
                }
                intent.repair
            }
            _ => return Err("This restore requires its ownership-specific workflow".into()),
        };
        repair.validate_record()?;
        let inverse: InverseOp =
            serde_json::from_value(row.inverse.clone().ok_or("Restore source has no inverse")?)
                .map_err(|error| error.to_string())?;
        let InverseOp::RestoreBackup {
            path,
            pre_fingerprint,
            post_fingerprint: Some(post),
        } = inverse
        else {
            return Err("Restore requires a completed document inverse".into());
        };
        let selected = crate::skill_deployment::parse_deployment_id(&repair.deployment_id)
            .ok_or("Invalid restore deployment")?;
        if path != repair.path.join("SKILL.md")
            || row.skill != repair.name
            || row.scope.as_deref() != Some(selected.scope.as_str())
            || row.project_path != selected.project_path
            || row.harness.is_some()
            || [&pre_fingerprint, &post].iter().any(|hash| {
                hash.len() != 64
                    || !hash
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
        {
            return Err("Restore source does not match its deployment".into());
        }
        if row.kind == "restore" {
            let intent: DirectRestoreIntent =
                serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?;
            if intent.before != pre_fingerprint || intent.after != post {
                return Err("Restore fingerprints do not match the inverse".into());
            }
        } else if post != fingerprint_regular_bytes(repair.proposed_content.as_bytes()) {
            return Err("Repair inverse does not match proposed content".into());
        }
        Ok(Self {
            row: row.clone(),
            repair,
            before: pre_fingerprint,
            after: post,
        })
    }
}

pub struct PreparedDirectRestore<'scope> {
    pub(crate) source: DirectRestoreSource,
    pub(crate) current: Vec<u8>,
    pub(crate) backup: VerifiedRepairBackup,
    pub(crate) lease: FinalizedWriteLease<'scope>,
}

fn pending_row(id: &str, ts: String, draft: EventDraft) -> EventRow {
    EventRow {
        id: id.into(),
        ts,
        kind: draft.kind,
        skill: draft.skill,
        harness: draft.harness,
        scope: draft.scope,
        project_path: draft.project_path,
        payload: draft.payload,
        inverse: draft.inverse,
        backup_dir: draft.backup_dir,
        status: "pending".into(),
        reverted_by: None,
        restorable: draft.restorable,
    }
}

impl RepairEventWorker<'_> {
    pub fn restore(
        &self,
        prepared: PreparedDirectRestore<'_>,
        restore_id: &str,
    ) -> Result<RepairExecutionReceipt, RepairExecutionError> {
        self.restore_with(prepared, restore_id, |_| {})
    }

    pub(crate) fn restore_with(
        &self,
        prepared: PreparedDirectRestore<'_>,
        restore_id: &str,
        mut checkpoint: impl FnMut(RepairExecutionStage),
    ) -> Result<RepairExecutionReceipt, RepairExecutionError> {
        let fail = |stage, message| RepairExecutionError {
            event_id: restore_id.into(),
            stage,
            message,
        };
        if restore_id == prepared.source.row.id {
            return Err(fail(
                RepairExecutionStage::Prepare,
                "Restore ID must differ from its source".into(),
            ));
        }
        let PreparedDirectRestore {
            source,
            current,
            backup,
            lease,
        } = prepared;
        let document = source.repair.path.join("SKILL.md");
        let before = fingerprint_regular_bytes(&current);
        let after = fingerprint_regular_bytes(backup.original());
        let intent = DirectRestoreIntent {
            target_event: source.row.id.clone(),
            repair: source.repair.clone(),
            before: before.clone(),
            after: after.clone(),
        };
        let inverse = |post| {
            serde_json::to_value(InverseOp::RestoreBackup {
                path: document.clone(),
                pre_fingerprint: before.clone(),
                post_fingerprint: post,
            })
            .map_err(|error| error.to_string())
        };
        let draft = EventDraft {
            kind: "restore".into(),
            skill: source.row.skill.clone(),
            harness: source.row.harness.clone(),
            scope: source.row.scope.clone(),
            project_path: source.row.project_path.clone(),
            payload: serde_json::to_value(intent)
                .map_err(|error| fail(RepairExecutionStage::Intent, error.to_string()))?,
            inverse: Some(
                inverse(None).map_err(|message| fail(RepairExecutionStage::Intent, message))?,
            ),
            backup_dir: Some(format!("backups/{restore_id}")),
            restorable: true,
        };
        let timestamp = chrono::Utc::now().to_rfc3339();
        let request = PreparedEventExchange::record_restore(
            "restore-record".into(),
            restore_command_id(restore_id, "record"),
            restore_id.into(),
            source.row.clone(),
            timestamp.clone(),
            draft.clone(),
        )
        .map_err(|message| fail(RepairExecutionStage::Prepare, message))?;
        let (mut lease, result) = self.prepare(lease, restore_id);
        result.map_err(|message| fail(RepairExecutionStage::Prepare, message))?;
        backup
            .revalidate(&lease)
            .map_err(|message| fail(RepairExecutionStage::Backup, message))?;
        let selected = BackupSourceRoot::bind(&source.repair.path)
            .and_then(|root| root.select(OsStr::new("SKILL.md")))
            .map_err(|error| fail(RepairExecutionStage::Backup, error.to_string()))?;
        let state = BackupStateRoot::bind(self.state_root)
            .map_err(|error| fail(RepairExecutionStage::Backup, error.to_string()))?;
        let manifest = lease
            .backup_documents(
                &state,
                restore_id,
                vec![selected],
                BackupCopyLimits {
                    max_bytes: current.len() as u64,
                    max_entries: 1,
                    max_depth: 0,
                },
            )
            .map_err(|message| fail(RepairExecutionStage::Backup, message))?;
        if manifest
            .entries
            .get(document.to_str().ok_or_else(|| {
                fail(RepairExecutionStage::Backup, "Invalid document path".into())
            })?)
            .is_none_or(|entry| entry.fingerprint != before)
        {
            return Err(fail(
                RepairExecutionStage::Backup,
                "Restore backup does not match validated current bytes".into(),
            ));
        }
        let (returned, result) = self.commit(lease, Ok(request));
        lease = returned;
        result.map_err(|message| fail(RepairExecutionStage::Intent, message))?;
        checkpoint(RepairExecutionStage::Intent);
        self.ensure_running()
            .map_err(|message| fail(RepairExecutionStage::Document, message))?;
        backup
            .revalidate(&lease)
            .map_err(|message| fail(RepairExecutionStage::Backup, message))?;
        SkillDocumentTarget::bind(&source.repair.path)
            .and_then(|target| {
                target
                    .replace(&mut lease, &current, backup.original())
                    .map_err(|error| error.to_string())
            })
            .map_err(|message| fail(RepairExecutionStage::Document, message))?;
        checkpoint(RepairExecutionStage::Document);
        let mut claimed = source.row;
        claimed.reverted_by = Some(restore_id.into());
        let request = PreparedEventExchange::finish_restore(
            "restore-finish".into(),
            restore_command_id(restore_id, "finish"),
            claimed,
            pending_row(restore_id, timestamp, draft),
            EventStatus::Done,
            Some(
                inverse(Some(after))
                    .map_err(|message| fail(RepairExecutionStage::Finish, message))?,
            ),
        );
        let (_lease, result) = self.commit(lease, request);
        result.map_err(|message| fail(RepairExecutionStage::Finish, message))?;
        Ok(RepairExecutionReceipt {
            event_id: restore_id.into(),
            deployment_id: source.repair.deployment_id,
        })
    }
}

fn restore_command_id(id: &str, phase: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(format!("direct-restore-v1:{id}:{phase}"))
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub struct PreparedDirectRestoreRecovery<'scope> {
    prepared: PreparedDirectRestore<'scope>,
    restore: EventRow,
    overwritten: VerifiedRepairBackup,
    applied: bool,
}

impl crate::skill_service::ScopedSkillService {
    pub fn prepare_direct_restore_recovery(
        &mut self,
        source: &EventRow,
        restore: &EventRow,
        state_root: &Path,
        timeout: Option<std::time::Duration>,
        cancellation: crate::skill_service::CancellationToken,
    ) -> Result<PreparedDirectRestoreRecovery<'_>, crate::skill_service::WritePreparationError>
    {
        let invalid = crate::skill_service::WritePreparationError::InvalidRepairSelection;
        let source = DirectRestoreSource::from_row(source, Some(&restore.id)).map_err(invalid)?;
        if !matches!(restore.status.as_str(), "pending" | "interrupted")
            || restore.kind != "restore"
            || restore.reverted_by.is_some()
            || !restore.restorable
            || restore.backup_dir.as_deref() != Some(format!("backups/{}", restore.id).as_str())
        {
            return Err(invalid(
                "Restore recovery requires an unresolved restore".into(),
            ));
        }
        let intent: DirectRestoreIntent = serde_json::from_value(restore.payload.clone())
            .map_err(|error| invalid(error.to_string()))?;
        if intent.target_event != source.row.id
            || intent.after != source.before
            || serde_json::to_value(&intent.repair).map_err(|error| invalid(error.to_string()))?
                != serde_json::to_value(&source.repair)
                    .map_err(|error| invalid(error.to_string()))?
            || restore.skill != source.row.skill
            || restore.harness != source.row.harness
            || restore.scope != source.row.scope
            || restore.project_path != source.row.project_path
        {
            return Err(invalid("Restore recovery source and intent differ".into()));
        }
        let expected_inverse = serde_json::to_value(InverseOp::RestoreBackup {
            path: source.repair.path.join("SKILL.md"),
            pre_fingerprint: intent.before.clone(),
            post_fingerprint: None,
        })
        .map_err(|error| invalid(error.to_string()))?;
        if restore.inverse.as_ref() != Some(&expected_inverse) {
            return Err(invalid("Restore recovery inverse changed".into()));
        }
        let prepared =
            self.prepare_direct_restore_target(source, state_root, timeout, cancellation)?;
        let overwritten = VerifiedRepairBackup::read_document(
            state_root,
            &restore.id,
            &prepared.source.repair.path.join("SKILL.md"),
            &intent.before,
            &prepared.lease,
        )
        .map_err(invalid)?;
        let current = fingerprint_regular_bytes(&prepared.current);
        let applied = if current == intent.after {
            true
        } else if current == intent.before {
            false
        } else {
            return Err(invalid(
                "Restore recovery refuses changed document bytes".into(),
            ));
        };
        Ok(PreparedDirectRestoreRecovery {
            prepared,
            restore: restore.clone(),
            overwritten,
            applied,
        })
    }
}

impl RepairEventWorker<'_> {
    pub fn recover_restore(
        &self,
        recovery: PreparedDirectRestoreRecovery<'_>,
    ) -> Result<crate::skill_repair_execution::RepairRecoveryOutcome, RepairExecutionError> {
        let fail = |stage, message| RepairExecutionError {
            event_id: recovery.restore.id.clone(),
            stage,
            message,
        };
        recovery
            .prepared
            .backup
            .revalidate(&recovery.prepared.lease)
            .map_err(|message| fail(RepairExecutionStage::Backup, message))?;
        recovery
            .overwritten
            .revalidate(&recovery.prepared.lease)
            .map_err(|message| fail(RepairExecutionStage::Backup, message))?;
        let document = recovery.prepared.source.repair.path.join("SKILL.md");
        let current = recovery
            .prepared
            .lease
            .read(&document, crate::skill_service::MAX_REPAIR_DOCUMENT_BYTES)
            .map_err(|error| fail(RepairExecutionStage::Recover, error.to_string()))?;
        if current != recovery.prepared.current {
            return Err(fail(
                RepairExecutionStage::Recover,
                "Restore recovery document changed after preparation".into(),
            ));
        }
        let status = if recovery.applied {
            EventStatus::Done
        } else {
            EventStatus::Failed
        };
        let inverse = serde_json::to_value(InverseOp::RestoreBackup {
            path: document,
            pre_fingerprint: fingerprint_regular_bytes(recovery.overwritten.original()),
            post_fingerprint: recovery
                .applied
                .then(|| fingerprint_regular_bytes(recovery.prepared.backup.original())),
        })
        .map_err(|error| fail(RepairExecutionStage::Finish, error.to_string()))?;
        let request = PreparedEventExchange::finish_restore(
            "restore-recover".into(),
            restore_command_id(&recovery.restore.id, "recover"),
            recovery.prepared.source.row,
            recovery.restore.clone(),
            status,
            Some(inverse),
        );
        let (_lease, result) = self.commit(recovery.prepared.lease, request);
        result.map_err(|message| fail(RepairExecutionStage::Finish, message))?;
        Ok(if recovery.applied {
            crate::skill_repair_execution::RepairRecoveryOutcome::Applied
        } else {
            crate::skill_repair_execution::RepairRecoveryOutcome::NotApplied
        })
    }
}
