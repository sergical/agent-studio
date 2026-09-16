//! Consistency checks for saved repair events. This value is not authority to
//! access its backup or target; those require independently bound capabilities.
use crate::{
    skill_backup_reservation::valid_id,
    skill_deployment::parse_deployment_id,
    skill_event::{EventRow, InverseOp},
    skill_event_store::fingerprint_regular_bytes,
    skill_repair_intent::FrontmatterRepairIntent,
};

pub struct RepairRecoveryEvent {
    snapshot: EventRow,
    id: String,
    status: String,
    intent: FrontmatterRepairIntent,
    original_file_fingerprint: String,
}

impl RepairRecoveryEvent {
    pub fn from_row(row: &EventRow) -> Result<Self, String> {
        if !valid_id(&row.id)
            || row.kind != "repair_skill_frontmatter"
            || !matches!(row.status.as_str(), "pending" | "interrupted")
            || row.reverted_by.is_some()
            || !row.restorable
        {
            return Err("Event is not an eligible interrupted repair".into());
        }
        let intent: FrontmatterRepairIntent =
            serde_json::from_value(row.payload.clone()).map_err(|_| "Malformed repair intent")?;
        intent.validate_record()?;
        let selected =
            parse_deployment_id(&intent.deployment_id).ok_or("Invalid repair deployment")?;
        if row.skill != intent.name
            || row.scope.as_deref() != Some(selected.scope.as_str())
            || row.project_path != selected.project_path
            || row.harness.is_some()
            || row.backup_dir.as_deref() != Some(format!("backups/{}", row.id).as_str())
        {
            return Err("Repair event target or backup reference does not match its intent".into());
        }
        let inverse =
            serde_json::from_value(row.inverse.clone().ok_or("Repair event has no inverse")?)
                .map_err(|_| "Malformed repair inverse")?;
        let InverseOp::RestoreBackup {
            path,
            pre_fingerprint,
            post_fingerprint,
        } = inverse
        else {
            return Err("Repair event does not have a document backup inverse".into());
        };
        if path != intent.path.join("SKILL.md")
            || pre_fingerprint.len() != 64
            || !pre_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            || post_fingerprint.is_some_and(|hash| {
                hash != fingerprint_regular_bytes(intent.proposed_content.as_bytes())
            })
        {
            return Err("Repair inverse target or fingerprint is inconsistent".into());
        }
        Ok(Self {
            snapshot: row.clone(),
            id: row.id.clone(),
            status: row.status.clone(),
            intent,
            original_file_fingerprint: pre_fingerprint,
        })
    }

    pub(crate) fn snapshot(&self) -> &EventRow {
        &self.snapshot
    }

    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn status(&self) -> &str {
        &self.status
    }
    pub fn intent(&self) -> &FrontmatterRepairIntent {
        &self.intent
    }
    pub fn original_file_fingerprint(&self) -> &str {
        &self.original_file_fingerprint
    }
}

pub struct CopyRepairRecoveryEvent {
    snapshot: EventRow,
    intent: crate::skill_copy_repair::CopyRepairIntent,
}

fn validated_copy_intent(
    row: &EventRow,
) -> Result<crate::skill_copy_repair::CopyRepairIntent, String> {
    if !valid_id(&row.id)
        || !matches!(
            row.kind.as_str(),
            "repair_copy_frontmatter" | "redo_copy_frontmatter"
        )
        || row.restorable
        || row.inverse.is_some()
    {
        return Err("Event is not a copy repair".into());
    }
    let intent: crate::skill_copy_repair::CopyRepairIntent = if row.kind == "redo_copy_frontmatter"
    {
        let redo: crate::skill_copy_repair::CopyRepairRedoIntent =
            serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?;
        redo.validate_record()?;
        if redo.source_event == row.id
            || redo.undo_event == row.id
            || serde_json::to_value(&redo).map_err(|error| error.to_string())? != row.payload
        {
            return Err("Copy redo source has inconsistent history metadata".into());
        }
        redo.repair
    } else {
        serde_json::from_value(row.payload.clone()).map_err(|error| error.to_string())?
    };
    intent.validate_record()?;
    let selected =
        parse_deployment_id(&intent.document.deployment_id).ok_or("Invalid copy deployment")?;
    if row.skill != intent.document.name
        || row.scope.as_deref() != Some(selected.scope.as_str())
        || row.project_path != selected.project_path
        || row.harness.is_some()
        || row.backup_dir.as_deref() != Some(format!("backups/{}", row.id).as_str())
    {
        return Err("Copy repair event does not match its intent".into());
    }
    Ok(intent)
}

impl CopyRepairRecoveryEvent {
    pub fn from_row(row: &EventRow) -> Result<Self, String> {
        if row.kind != "repair_copy_frontmatter"
            || !matches!(row.status.as_str(), "pending" | "interrupted")
            || row.reverted_by.is_some()
        {
            return Err("Copy repair is not interrupted".into());
        }
        Ok(Self {
            snapshot: row.clone(),
            intent: validated_copy_intent(row)?,
        })
    }
    pub fn id(&self) -> &str {
        &self.snapshot.id
    }
    pub fn intent(&self) -> &crate::skill_copy_repair::CopyRepairIntent {
        &self.intent
    }
    pub(crate) fn snapshot(&self) -> &EventRow {
        &self.snapshot
    }
}

pub struct CopyRepairUndoSource {
    snapshot: EventRow,
    intent: crate::skill_copy_repair::CopyRepairIntent,
}

impl CopyRepairUndoSource {
    pub fn from_row(row: &EventRow) -> Result<Self, String> {
        if row.status != "done" || row.reverted_by.is_some() {
            return Err("Copy undo requires a completed repair".into());
        }
        Ok(Self {
            snapshot: row.clone(),
            intent: validated_copy_intent(row)?,
        })
    }
    pub fn id(&self) -> &str {
        &self.snapshot.id
    }
    pub fn intent(&self) -> &crate::skill_copy_repair::CopyRepairIntent {
        &self.intent
    }
    pub(crate) fn snapshot(&self) -> &EventRow {
        &self.snapshot
    }
}

pub struct CopyUndoRecoveryEvent {
    source: EventRow,
    undo: EventRow,
    intent: crate::skill_copy_repair::CopyRepairIntent,
}

fn validated_copy_undo_intent(
    source: &EventRow,
    undo: &EventRow,
) -> Result<crate::skill_copy_repair::CopyRepairIntent, String> {
    let intent = validated_copy_intent(source)?;
    if source.status != "done"
        || source.reverted_by.as_deref() != Some(undo.id.as_str())
        || !valid_id(&undo.id)
        || undo.id == source.id
        || undo.kind != "undo_copy_frontmatter"
        || undo.reverted_by.is_some()
        || undo.restorable
        || undo.inverse.is_some()
        || undo.skill != source.skill
        || undo.harness != source.harness
        || undo.scope != source.scope
        || undo.project_path != source.project_path
        || undo.backup_dir.as_deref() != Some(format!("backups/{}", undo.id).as_str())
        || undo.payload != serde_json::json!({"target_event": source.id, "repair": intent})
    {
        return Err("Copy undo event does not match its source claim and repair".into());
    }
    Ok(intent)
}

impl CopyUndoRecoveryEvent {
    pub fn from_rows(source: &EventRow, undo: &EventRow) -> Result<Self, String> {
        if !matches!(undo.status.as_str(), "pending" | "interrupted") {
            return Err("Copy undo is not interrupted".into());
        }
        let intent = validated_copy_undo_intent(source, undo)?;
        Ok(Self {
            source: source.clone(),
            undo: undo.clone(),
            intent,
        })
    }
    pub fn source(&self) -> &EventRow {
        &self.source
    }
    pub fn undo(&self) -> &EventRow {
        &self.undo
    }
    pub fn intent(&self) -> &crate::skill_copy_repair::CopyRepairIntent {
        &self.intent
    }
}

pub struct CopyRepairRedoSource {
    source: EventRow,
    undo: EventRow,
    intent: crate::skill_copy_repair::CopyRepairIntent,
}

impl CopyRepairRedoSource {
    pub fn from_rows(source: &EventRow, undo: &EventRow) -> Result<Self, String> {
        if undo.status != "done" {
            return Err("Copy redo requires a completed undo".into());
        }
        let intent = validated_copy_undo_intent(source, undo)?;
        Ok(Self {
            source: source.clone(),
            undo: undo.clone(),
            intent,
        })
    }
    pub fn source(&self) -> &EventRow {
        &self.source
    }
    pub fn undo(&self) -> &EventRow {
        &self.undo
    }
    pub fn intent(&self) -> &crate::skill_copy_repair::CopyRepairIntent {
        &self.intent
    }
}

pub struct CopyRedoRecoveryEvent {
    source: EventRow,
    undo: EventRow,
    redo: EventRow,
    intent: crate::skill_copy_repair::CopyRepairRedoIntent,
    prior_intent: crate::skill_copy_repair::CopyRepairIntent,
}

impl CopyRedoRecoveryEvent {
    pub fn from_rows(source: &EventRow, undo: &EventRow, redo: &EventRow) -> Result<Self, String> {
        if !valid_id(&redo.id)
            || redo.id == source.id
            || redo.id == undo.id
            || undo.reverted_by.as_deref() != Some(redo.id.as_str())
            || redo.kind != "redo_copy_frontmatter"
            || !matches!(redo.status.as_str(), "pending" | "interrupted")
            || redo.reverted_by.is_some()
            || redo.restorable
            || redo.inverse.is_some()
            || redo.skill != source.skill
            || redo.harness != source.harness
            || redo.scope != source.scope
            || redo.project_path != source.project_path
            || redo.backup_dir.as_deref() != Some(format!("backups/{}", redo.id).as_str())
        {
            return Err("Copy redo recovery event does not match its claimed history".into());
        }
        let mut unclaimed_undo = undo.clone();
        unclaimed_undo.reverted_by = None;
        let prior = CopyRepairRedoSource::from_rows(source, &unclaimed_undo)?;
        let intent: crate::skill_copy_repair::CopyRepairRedoIntent =
            serde_json::from_value(redo.payload.clone()).map_err(|error| error.to_string())?;
        intent.validate_source(&prior)?;
        if serde_json::to_value(&intent).map_err(|error| error.to_string())? != redo.payload {
            return Err("Copy redo recovery intent has unexpected fields".into());
        }
        Ok(Self {
            source: source.clone(),
            undo: undo.clone(),
            redo: redo.clone(),
            intent,
            prior_intent: prior.intent().clone(),
        })
    }
    pub fn source(&self) -> &EventRow {
        &self.source
    }
    pub fn undo(&self) -> &EventRow {
        &self.undo
    }
    pub fn redo(&self) -> &EventRow {
        &self.redo
    }
    pub fn intent(&self) -> &crate::skill_copy_repair::CopyRepairRedoIntent {
        &self.intent
    }
    pub fn prior_intent(&self) -> &crate::skill_copy_repair::CopyRepairIntent {
        &self.prior_intent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        skill_deployment::{deployment_id, SkillDestination},
        skill_frontmatter_repair::{preview_frontmatter_repair, FrontmatterRepairApplyMode},
        skill_inventory::Deployment,
        skill_ownership::LifecycleOwnerKind,
    };
    use std::path::Path;

    fn row() -> EventRow {
        let path = Path::new("/fixture/alpha");
        let deployment = Deployment {
            id: deployment_id(
                "alpha",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                path,
            ),
            path: path.to_string_lossy().into_owned(),
            scope: "global".into(),
            owner_kind: LifecycleOwnerKind::Manual,
            ..Deployment::default()
        };
        let original = b"---\nname: alpha\ndescription: this: fixture\n---\nbody\n";
        let preview = preview_frontmatter_repair(&deployment, original).unwrap();
        let intent = FrontmatterRepairIntent::from_preview(
            &preview,
            FrontmatterRepairApplyMode::ApplyFix,
            None,
        )
        .unwrap();
        EventRow {
            id: "repair".into(),
            ts: "fixture".into(),
            kind: "repair_skill_frontmatter".into(),
            skill: "alpha".into(),
            harness: None,
            scope: Some("global".into()),
            project_path: None,
            payload: serde_json::to_value(intent).unwrap(),
            inverse: Some(
                serde_json::to_value(InverseOp::RestoreBackup {
                    path: path.join("SKILL.md"),
                    pre_fingerprint: fingerprint_regular_bytes(original),
                    post_fingerprint: None,
                })
                .unwrap(),
            ),
            backup_dir: Some("backups/repair".into()),
            status: "pending".into(),
            reverted_by: None,
            restorable: true,
        }
    }

    #[test]
    fn accepts_pending_and_interrupted_rows_with_matching_inverse() {
        for status in ["pending", "interrupted"] {
            let mut row = row();
            row.status = status.into();
            let event = RepairRecoveryEvent::from_row(&row).unwrap();
            assert_eq!(event.id(), "repair");
            assert_eq!(event.status(), status);
            assert_eq!(event.intent().name, "alpha");
            assert_eq!(event.original_file_fingerprint().len(), 64);
            row.inverse.as_mut().unwrap()["post_fingerprint"] = serde_json::json!(
                fingerprint_regular_bytes(event.intent().proposed_content.as_bytes())
            );
            RepairRecoveryEvent::from_row(&row).unwrap();
        }
    }

    #[test]
    fn rejects_mismatched_event_metadata_inverse_and_backup_labels() {
        for change in [
            "id",
            "kind",
            "status",
            "claimed",
            "restorable",
            "skill",
            "scope",
            "project",
            "harness",
            "backup",
            "inverse",
            "path",
            "pre",
            "post",
        ] {
            let mut row = row();
            match change {
                "id" => row.id = "../escape".into(),
                "kind" => row.kind = "remove_skill".into(),
                "status" => row.status = "done".into(),
                "claimed" => row.reverted_by = Some("restore".into()),
                "restorable" => row.restorable = false,
                "skill" => row.skill = "other".into(),
                "scope" => row.scope = Some("project".into()),
                "project" => row.project_path = Some("/other".into()),
                "harness" => row.harness = Some("codex".into()),
                "backup" => row.backup_dir = Some("backups/../other".into()),
                "inverse" => row.inverse = None,
                "path" => {
                    row.inverse.as_mut().unwrap()["path"] = serde_json::json!("/other/SKILL.md")
                }
                "pre" => {
                    row.inverse.as_mut().unwrap()["pre_fingerprint"] = serde_json::json!("bad")
                }
                "post" => {
                    row.inverse.as_mut().unwrap()["post_fingerprint"] = serde_json::json!("bad")
                }
                _ => unreachable!(),
            }
            assert!(
                RepairRecoveryEvent::from_row(&row).is_err(),
                "case: {change}"
            );
        }
    }
}
