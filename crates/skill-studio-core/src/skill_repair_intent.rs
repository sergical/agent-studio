//! Persisted repair data. Validation establishes consistency, not path authority.
use crate::{
    skill_deployment::parse_deployment_id,
    skill_fork_registry::ForkRegistry,
    skill_frontmatter_repair::{
        content_fingerprint, propose_colon_scalar_repair, FrontmatterRepairApplyMode,
        FrontmatterRepairPreview,
    },
};
use serde::{Deserialize, Serialize};
use std::path::{Component, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontmatterRepairIntent {
    pub deployment_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proposal_id: Option<String>,
    pub name: String,
    pub path: PathBuf,
    pub expected_content_fingerprint: String,
    pub proposed_content: String,
    pub proposed_content_fingerprint: String,
    pub mode: FrontmatterRepairApplyMode,
    pub managed_update_warning: bool,
    pub fork_registry_before: Option<ForkRegistry>,
}

impl FrontmatterRepairIntent {
    /// The caller must supply a fresh authorized preview under its write lease.
    pub fn from_preview(
        preview: &FrontmatterRepairPreview,
        mode: FrontmatterRepairApplyMode,
        fork_registry_before: Option<ForkRegistry>,
    ) -> Result<Self, String> {
        if !preview.allowed_apply_modes.contains(&mode) {
            return Err("Repair mode is not allowed by the preview".into());
        }
        let selected = parse_deployment_id(&preview.deployment_id)
            .ok_or("Repair intent has an invalid deployment ID")?;
        if selected.scope != preview.scope {
            return Err("Repair intent scope does not match its deployment".into());
        }
        let intent = Self {
            deployment_id: preview.deployment_id.clone(),
            proposal_id: Some(preview.proposal_id.clone()),
            name: selected.name,
            path: PathBuf::from(&preview.path),
            expected_content_fingerprint: preview.expected_content_fingerprint.clone(),
            proposed_content: preview.proposed_content.clone(),
            proposed_content_fingerprint: content_fingerprint(preview.proposed_content.as_bytes()),
            mode,
            managed_update_warning: mode == FrontmatterRepairApplyMode::FixInstalledCopy,
            fork_registry_before,
        };
        intent.validate_record()?;
        intent.validate_original(preview.original_content.as_bytes())?;
        Ok(intent)
    }

    /// Run before using persisted fields. Bound scope and ownership checks are
    /// still required before any filesystem operation, including recovery reads.
    pub fn validate_record(&self) -> Result<(), String> {
        let selected = parse_deployment_id(&self.deployment_id)
            .ok_or("Repair intent has an invalid deployment ID")?;
        if selected.name != self.name
            || selected.lexical_path != self.path
            || self.name.is_empty()
            || self.name == "."
            || self.name == ".."
            || self.name.contains(['/', '\\', '\0'])
            || !self.path.is_absolute()
            || self
                .path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
        {
            return Err("Repair intent identity and path do not match".into());
        }
        let hash = self
            .expected_content_fingerprint
            .strip_prefix("sha256:")
            .ok_or("Repair intent has an invalid original fingerprint")?;
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err("Repair intent has an invalid original fingerprint".into());
        }
        if content_fingerprint(self.proposed_content.as_bytes())
            != self.proposed_content_fingerprint
            || self.expected_content_fingerprint == self.proposed_content_fingerprint
        {
            return Err("Repair intent proposed content does not match its fingerprint".into());
        }
        if (self.mode == FrontmatterRepairApplyMode::ForkAndFix)
            != self.fork_registry_before.is_some()
        {
            return Err("Repair intent has an inconsistent fork rollback snapshot".into());
        }
        Ok(())
    }

    /// Direct repair preserves ownership. ForkAndFix needs a separate proof of
    /// its ownership transition; legacy intents have no proposal binding.
    pub fn validate_direct_recovery_deployment(
        &self,
        deployment: &crate::skill_inventory::Deployment,
    ) -> Result<(), String> {
        self.validate_record()?;
        if self.mode == FrontmatterRepairApplyMode::ForkAndFix {
            return Err("Fork repair requires ownership-transition validation".into());
        }
        let saved = self
            .proposal_id
            .as_deref()
            .ok_or("Legacy repair intent has no ownership proposal binding")?;
        if deployment.id != self.deployment_id
            || std::path::Path::new(&deployment.path) != self.path
            || !crate::skill_frontmatter_repair::allowed_repair_modes(deployment)
                .contains(&self.mode)
            || saved
                != crate::skill_frontmatter_repair::proposal_id(
                    deployment,
                    &self.expected_content_fingerprint,
                    &self.proposed_content,
                )
        {
            return Err("Repair ownership or proposal changed; recovery requires review".into());
        }
        Ok(())
    }

    /// Edits only this repair's fork entry in fresh registry data. The caller
    /// must coordinate the read/write and separately authorize artifact cleanup.
    pub fn roll_back_fork_record(&self, current: &mut ForkRegistry) -> Result<bool, String> {
        self.validate_record()?;
        let before = self
            .fork_registry_before
            .as_ref()
            .ok_or("Repair intent has no fork rollback snapshot")?;
        let previous = before.forks.get(&self.name);
        let live = current.forks.get(&self.name);
        if live == previous {
            return Ok(false);
        }
        if !live.is_some_and(|record| {
            record.deployment_id == self.deployment_id && record.skill_dir == self.path
        }) {
            return Err("Repair fork entry changed; rollback requires review".into());
        }
        match previous {
            Some(record) => {
                current.forks.insert(self.name.clone(), record.clone());
            }
            None => {
                current.forks.remove(&self.name);
            }
        }
        Ok(true)
    }

    /// Proves that the stored proposal is the deterministic repair of these
    /// original bytes. It does not authorize replacement or ownership changes.
    pub fn validate_original(&self, bytes: &[u8]) -> Result<(), String> {
        if content_fingerprint(bytes) != self.expected_content_fingerprint {
            return Err("Repair original content changed".into());
        }
        let original = std::str::from_utf8(bytes).map_err(|_| "Repair original is not UTF-8")?;
        let (proposed, _) = propose_colon_scalar_repair(original)?;
        if proposed != self.proposed_content {
            return Err("Repair intent is not the proposed deterministic repair".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        skill_deployment::{deployment_id, SkillDestination},
        skill_frontmatter_repair::preview_frontmatter_repair,
        skill_inventory::Deployment,
        skill_ownership::LifecycleOwnerKind,
    };

    fn preview() -> FrontmatterRepairPreview {
        let path = PathBuf::from("/fixture/sample");
        let deployment = Deployment {
            id: deployment_id(
                "sample",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                &path,
            ),
            path: path.to_str().unwrap().into(),
            scope: "global".into(),
            owner_kind: LifecycleOwnerKind::Manual,
            ..Deployment::default()
        };
        preview_frontmatter_repair(
            &deployment,
            b"---\nname: sample\ndescription: this: fixture\n---\nbody\n",
        )
        .unwrap()
    }

    #[test]
    fn intent_preserves_wire_fields_and_validates_original() {
        let preview = preview();
        let intent = FrontmatterRepairIntent::from_preview(
            &preview,
            FrontmatterRepairApplyMode::ApplyFix,
            None,
        )
        .unwrap();
        let encoded = serde_json::to_value(&intent).unwrap();
        let mut keys = encoded
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        keys.sort();
        assert_eq!(
            keys,
            [
                "deployment_id",
                "expected_content_fingerprint",
                "fork_registry_before",
                "managed_update_warning",
                "mode",
                "name",
                "path",
                "proposal_id",
                "proposed_content",
                "proposed_content_fingerprint"
            ]
        );
        assert_eq!(encoded["mode"], "apply-fix");
        let decoded: FrontmatterRepairIntent = serde_json::from_value(encoded).unwrap();
        decoded.validate_record().unwrap();
        decoded
            .validate_original(preview.original_content.as_bytes())
            .unwrap();
        assert!(decoded.validate_original(b"changed").is_err());
    }

    #[test]
    fn rejects_inconsistent_persisted_identity_content_and_fork_state() {
        let preview = preview();
        let original = FrontmatterRepairIntent::from_preview(
            &preview,
            FrontmatterRepairApplyMode::ApplyFix,
            None,
        )
        .unwrap();
        for change in ["id", "path", "name", "hash", "content", "snapshot", "fork"] {
            let mut intent = original.clone();
            match change {
                "id" => intent.deployment_id = "invalid".into(),
                "path" => intent.path = "/other/sample".into(),
                "name" => intent.name = "../other".into(),
                "hash" => intent.expected_content_fingerprint = "sha256:bad".into(),
                "content" => intent.proposed_content.push_str("changed"),
                "snapshot" => intent.fork_registry_before = Some(ForkRegistry::default()),
                "fork" => intent.mode = FrontmatterRepairApplyMode::ForkAndFix,
                _ => unreachable!(),
            }
            assert!(intent.validate_record().is_err(), "case: {change}");
        }
        let mut tampered = original;
        tampered.proposed_content.push_str("injected body");
        tampered.proposed_content_fingerprint =
            content_fingerprint(tampered.proposed_content.as_bytes());
        tampered.validate_record().unwrap();
        assert!(tampered
            .validate_original(preview.original_content.as_bytes())
            .is_err());
    }

    #[test]
    fn fork_rollback_preserves_unrelated_state_and_refuses_repointing() {
        use crate::skill_fork_registry::{ForkRecord, OriginTool};
        let mut preview = preview();
        preview.allowed_apply_modes = vec![FrontmatterRepairApplyMode::ForkAndFix];
        let record = ForkRecord {
            deployment_id: preview.deployment_id.clone(),
            skill_dir: PathBuf::from(&preview.path),
            forked_at: "now".into(),
            origin_tool: OriginTool::SkillsSh,
            origin_source: "owner/repo".into(),
            repo: "owner/repo".into(),
            path: "skills/sample".into(),
            declared_ref: None,
            base_commit: "new".into(),
        };
        for had_previous in [false, true] {
            let mut before = ForkRegistry::default();
            if had_previous {
                let mut old = record.clone();
                old.base_commit = "old".into();
                before.forks.insert("sample".into(), old);
            }
            let intent = FrontmatterRepairIntent::from_preview(
                &preview,
                FrontmatterRepairApplyMode::ForkAndFix,
                Some(before.clone()),
            )
            .unwrap();
            let mut current = before.clone();
            current.forks.insert("sample".into(), record.clone());
            current.forks.insert("unrelated".into(), record.clone());
            current.preferred_editor = Some("New editor".into());
            current
                .trusted_dotagents_sources
                .insert("new/source".into());
            let mut expected = current.clone();
            match before.forks.get("sample") {
                Some(old) => {
                    expected.forks.insert("sample".into(), old.clone());
                }
                None => {
                    expected.forks.remove("sample");
                }
            }
            for field in ["id", "path"] {
                let mut repointed = current.clone();
                let fork = repointed.forks.get_mut("sample").unwrap();
                if field == "id" {
                    fork.deployment_id = "another".into();
                } else {
                    fork.skill_dir = "/other/sample".into();
                }
                let original = serde_json::to_value(&repointed).unwrap();
                assert!(intent.roll_back_fork_record(&mut repointed).is_err());
                assert_eq!(serde_json::to_value(&repointed).unwrap(), original);
            }
            assert!(intent.roll_back_fork_record(&mut current).unwrap());
            assert_eq!(
                serde_json::to_value(&current).unwrap(),
                serde_json::to_value(&expected).unwrap()
            );
            assert!(!intent.roll_back_fork_record(&mut current).unwrap());
        }
    }

    #[test]
    fn direct_recovery_requires_current_proposal_and_preserves_legacy_decoding() {
        let preview = preview();
        let intent = FrontmatterRepairIntent::from_preview(
            &preview,
            FrontmatterRepairApplyMode::ApplyFix,
            None,
        )
        .unwrap();
        let mut deployment = Deployment {
            id: preview.deployment_id.clone(),
            path: preview.path.clone(),
            scope: preview.scope.clone(),
            owner_kind: LifecycleOwnerKind::Manual,
            ..Deployment::default()
        };
        intent
            .validate_direct_recovery_deployment(&deployment)
            .unwrap();
        deployment.owner_revision = Some("changed".into());
        assert!(intent
            .validate_direct_recovery_deployment(&deployment)
            .is_err());
        deployment.owner_revision = None;
        deployment.owner_id = Some("changed owner".into());
        assert!(intent
            .validate_direct_recovery_deployment(&deployment)
            .is_err());
        let mut legacy = serde_json::to_value(&intent).unwrap();
        legacy.as_object_mut().unwrap().remove("proposal_id");
        let legacy: FrontmatterRepairIntent = serde_json::from_value(legacy).unwrap();
        legacy.validate_record().unwrap();
        assert!(legacy
            .validate_direct_recovery_deployment(&deployment)
            .unwrap_err()
            .contains("Legacy"));
    }

    #[test]
    fn constructor_requires_matching_preview_content_and_mode() {
        let mut preview = preview();
        assert!(FrontmatterRepairIntent::from_preview(
            &preview,
            FrontmatterRepairApplyMode::FixInstalledCopy,
            None
        )
        .is_err());
        preview.proposed_content.push_str("injected body");
        assert!(FrontmatterRepairIntent::from_preview(
            &preview,
            FrontmatterRepairApplyMode::ApplyFix,
            None
        )
        .is_err());
    }
}
