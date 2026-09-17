//! Selected Copy location changes. Validated records grant no filesystem authority.
use crate::skill_deployment::{deployment_id, parse_deployment_id, InstallScope, SkillDestination};
use crate::skill_discovery::STUDIO_DISABLED_DIR_NAME;
use crate::skill_fork_registry::{CopyDeploymentRecord, ForkRegistry};
use serde::{Deserialize, Serialize};
use std::path::Component;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CopyMoveTransition {
    before: CopyDeploymentRecord,
    after: CopyDeploymentRecord,
}

impl CopyMoveTransition {
    pub fn new(before: CopyDeploymentRecord, enabled: bool) -> Result<Self, String> {
        if before.disabled != enabled {
            return Err("Copy visibility already matches the requested state".into());
        }
        let after = Self::moved_record(&before)?;
        Ok(Self { before, after })
    }

    fn moved_record(before: &CopyDeploymentRecord) -> Result<CopyDeploymentRecord, String> {
        let parsed = parse_deployment_id(&before.deployment_id)
            .ok_or("Invalid Copy move deployment identity")?;
        let scope = match before.scope {
            InstallScope::Global => "global",
            InstallScope::Project => "project",
        };
        if before.destination != SkillDestination::PerHarness
            || parsed.name != before.name
            || parsed.scope != scope
            || parsed.destination != before.destination
            || parsed.slot != before.slot
            || parsed.project_path != before.project_path
            || parsed.lexical_path != before.path
            || before.deployment_id
                != deployment_id(
                    &before.name,
                    scope,
                    before.destination,
                    &before.slot,
                    before.project_path.as_deref(),
                    &before.path,
                )
            || !before.path.is_absolute()
            || before
                .path
                .components()
                .any(|part| matches!(part, Component::ParentDir))
            || before.path.file_name().and_then(|name| name.to_str()) != Some(before.name.as_str())
            || before.name.is_empty()
            || before.name.contains(['/', '\\'])
            || matches!(before.name.as_str(), "." | "..")
            || (before.scope == InstallScope::Global && before.project_path.is_some())
            || (before.scope == InstallScope::Project && before.project_path.is_none())
        {
            return Err("Invalid Copy move ownership record".into());
        }
        let parent = before.path.parent().ok_or("Copy move path has no parent")?;
        let in_holding =
            parent.file_name().and_then(|name| name.to_str()) == Some(STUDIO_DISABLED_DIR_NAME);
        if in_holding != before.disabled {
            return Err("Copy move path does not match its disabled state".into());
        }
        let path = if before.disabled {
            parent
                .parent()
                .ok_or("Copy holding directory has no parent")?
                .join(&before.name)
        } else {
            parent.join(STUDIO_DISABLED_DIR_NAME).join(&before.name)
        };
        let mut after = before.clone();
        after.deployment_id = deployment_id(
            &before.name,
            scope,
            before.destination,
            &before.slot,
            before.project_path.as_deref(),
            &path,
        );
        after.path = path;
        after.disabled = !before.disabled;
        Ok(after)
    }

    pub fn validate(&self) -> Result<(), String> {
        if Self::moved_record(&self.before)? != self.after {
            return Err("Copy move changes fields outside its location and visibility".into());
        }
        Ok(())
    }

    pub fn before(&self) -> &CopyDeploymentRecord {
        &self.before
    }
    pub fn after(&self) -> &CopyDeploymentRecord {
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
        const MAX_REGISTRY_BYTES: usize = 8 * 1024 * 1024;
        if original.len() > MAX_REGISTRY_BYTES {
            return Err("Copy registry exceeds its limit".into());
        }
        let mut document: serde_json::Value =
            serde_json::from_slice(original).map_err(|error| error.to_string())?;
        let copies = document
            .get_mut("copies")
            .and_then(serde_json::Value::as_object_mut)
            .ok_or("Copy registry records are missing or invalid")?;
        let read_record = |value: &serde_json::Value| {
            serde_json::from_value::<CopyDeploymentRecord>(value.clone())
                .map_err(|error| error.to_string())
        };
        let source = copies.get(&expected.deployment_id);
        let target = copies.get(&replacement.deployment_id);
        if source.is_none() && target.map(read_record).transpose()?.as_ref() == Some(replacement) {
            return Ok(original.to_vec());
        }
        if source.map(read_record).transpose()?.as_ref() != Some(expected) || target.is_some() {
            return Err("Copy move ownership changed or its destination record is occupied".into());
        }
        let mut entry = copies
            .remove(&expected.deployment_id)
            .ok_or("Copy record is missing")?;
        let fields = entry
            .as_object_mut()
            .ok_or("Copy record is not an object")?;
        fields.insert(
            "deployment_id".into(),
            serde_json::to_value(&replacement.deployment_id).map_err(|error| error.to_string())?,
        );
        fields.insert(
            "path".into(),
            serde_json::to_value(&replacement.path).map_err(|error| error.to_string())?,
        );
        fields.insert(
            "disabled".into(),
            serde_json::Value::Bool(replacement.disabled),
        );
        copies.insert(replacement.deployment_id.clone(), entry);
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_REGISTRY_BYTES {
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
        let source = registry.copies.get(&expected.deployment_id);
        let target = registry.copies.get(&replacement.deployment_id);
        if source.is_none() && target == Some(replacement) {
            return Ok(false);
        }
        if source != Some(expected) || target.is_some() {
            return Err("Copy move ownership changed or its destination record is occupied".into());
        }
        registry.copies.remove(&expected.deployment_id);
        registry
            .copies
            .insert(replacement.deployment_id.clone(), replacement.clone());
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn record(project: bool) -> CopyDeploymentRecord {
        let root = if project {
            "/fixture/project"
        } else {
            "/fixture"
        };
        let path = PathBuf::from(root).join(".cursor/skills/sample");
        let project_path = project.then(|| root.to_string());
        CopyDeploymentRecord {
            deployment_id: deployment_id(
                "sample",
                if project { "project" } else { "global" },
                SkillDestination::PerHarness,
                "cursor",
                project_path.as_deref(),
                &path,
            ),
            name: "sample".into(),
            path,
            scope: if project {
                InstallScope::Project
            } else {
                InstallScope::Global
            },
            destination: SkillDestination::PerHarness,
            slot: "cursor".into(),
            project_path,
            content_hash: "a".repeat(64),
            disabled: false,
        }
    }

    #[test]
    fn move_and_reverse_are_idempotent_and_preserve_other_scopes() {
        for project in [false, true] {
            let before = record(project);
            let sibling = record(!project);
            let transition = CopyMoveTransition::new(before.clone(), false).unwrap();
            let mut registry = ForkRegistry::default();
            registry
                .copies
                .insert(before.deployment_id.clone(), before.clone());
            registry
                .copies
                .insert(sibling.deployment_id.clone(), sibling.clone());
            assert!(transition.apply(&mut registry).unwrap());
            assert!(!transition.apply(&mut registry).unwrap());
            assert_eq!(registry.copies.get(&sibling.deployment_id), Some(&sibling));
            assert!(transition.roll_back(&mut registry).unwrap());
            assert!(!transition.roll_back(&mut registry).unwrap());
            assert_eq!(registry.copies.get(&before.deployment_id), Some(&before));
            assert_eq!(registry.copies.len(), 2);
            let enable = CopyMoveTransition::new(transition.after().clone(), true).unwrap();
            assert_eq!(enable.after(), &before);
        }
    }

    #[test]
    fn changed_source_and_occupied_target_leave_registry_untouched() {
        let before = record(false);
        let transition = CopyMoveTransition::new(before.clone(), false).unwrap();
        for occupied in [false, true] {
            let mut registry = ForkRegistry::default();
            let mut source = before.clone();
            if !occupied {
                source.content_hash = "b".repeat(64);
            }
            registry.copies.insert(source.deployment_id.clone(), source);
            if occupied {
                registry.copies.insert(
                    transition.after().deployment_id.clone(),
                    transition.after().clone(),
                );
            }
            let original = serde_json::to_value(&registry).unwrap();
            assert!(transition.apply(&mut registry).is_err());
            assert_eq!(serde_json::to_value(registry).unwrap(), original);
        }
    }

    #[test]
    fn document_move_preserves_unknown_fields_and_unrelated_records() {
        for project in [false, true] {
            let before = record(project);
            let sibling = record(!project);
            let transition = CopyMoveTransition::new(before.clone(), false).unwrap();
            let mut selected = serde_json::to_value(&before).unwrap();
            selected["future_record_metadata"] = serde_json::json!({"keep": [1, 2, 3]});
            let document = serde_json::json!({
                "version": 4,
                "future_top_level": {"keep": true},
                "copies": {
                    before.deployment_id.clone(): selected,
                    sibling.deployment_id.clone(): sibling,
                    "unrecognized-owner": {"future_shape": true}
                }
            });
            let original = serde_json::to_vec(&document).unwrap();
            let moved = transition.apply_document(&original).unwrap();
            let moved_doc: serde_json::Value = serde_json::from_slice(&moved).unwrap();
            assert!(moved_doc["copies"].get(&before.deployment_id).is_none());
            assert_eq!(
                moved_doc["copies"][&transition.after().deployment_id]["future_record_metadata"],
                serde_json::json!({"keep": [1, 2, 3]})
            );
            assert_eq!(transition.apply_document(&moved).unwrap(), moved);
            let restored = transition.roll_back_document(&moved).unwrap();
            assert_eq!(
                serde_json::from_slice::<serde_json::Value>(&restored).unwrap(),
                document
            );
            assert_eq!(transition.roll_back_document(&restored).unwrap(), restored);
        }
    }

    #[test]
    fn document_move_rejects_changed_missing_and_duplicate_selected_records() {
        let before = record(false);
        let transition = CopyMoveTransition::new(before.clone(), false).unwrap();
        for conflict in ["changed", "missing", "occupied", "invalid"] {
            let mut document =
                serde_json::json!({"copies": {before.deployment_id.clone(): before}});
            match conflict {
                "changed" => {
                    document["copies"][&before.deployment_id]["content_hash"] =
                        serde_json::json!("b".repeat(64))
                }
                "missing" => {
                    document["copies"]
                        .as_object_mut()
                        .unwrap()
                        .remove(&before.deployment_id);
                }
                "occupied" => {
                    document["copies"][&transition.after().deployment_id] =
                        serde_json::to_value(transition.after()).unwrap();
                }
                _ => document["copies"] = serde_json::json!([]),
            }
            let original = serde_json::to_vec(&document).unwrap();
            assert!(transition.apply_document(&original).is_err(), "{conflict}");
        }
        assert!(transition.apply_document(b"not json").is_err());
        assert!(transition
            .apply_document(&vec![b' '; 8 * 1024 * 1024 + 1])
            .is_err());
    }

    #[test]
    fn deserialized_transition_cannot_change_content_or_scope() {
        let transition = CopyMoveTransition::new(record(false), false).unwrap();
        for field in ["content_hash", "path", "project_path"] {
            let mut value = serde_json::to_value(&transition).unwrap();
            value["after"][field] = serde_json::json!("/unrelated");
            let tampered: CopyMoveTransition = serde_json::from_value(value).unwrap();
            assert!(tampered.validate().is_err());
        }
        assert!(CopyMoveTransition::new(record(false), true).is_err());
        let mut invalid = record(false);
        invalid.disabled = true;
        assert!(CopyMoveTransition::new(invalid, true).is_err());
    }
}
