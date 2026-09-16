//! Exact Fork ownership projection for one selected deployment.
use crate::skill_deployment::{deployment_id, SkillDestination};
use crate::skill_fork_registry::{deployment_trial_key, trial_key, ForkRecord, TrialScope};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_REGISTRY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForkRegistryTransition {
    name: String,
    record: ForkRecord,
    trials_before: BTreeMap<String, Value>,
}

impl ForkRegistryTransition {
    pub fn new(name: String, record: ForkRecord, registry: &[u8]) -> Result<Self, String> {
        let document = parse_registry(registry)?;
        if entry(&document, "forks", &name)?.is_some() {
            return Err("Fork record already exists".into());
        }
        let mut transition = Self {
            name,
            record,
            trials_before: BTreeMap::new(),
        };
        for key in transition.trial_keys() {
            if let Some(value) = entry(&document, "trials", &key)? {
                transition.trials_before.insert(key, value.clone());
            }
        }
        transition.validate()?;
        Ok(transition)
    }

    pub fn record(&self) -> &ForkRecord {
        &self.record
    }

    fn trial_keys(&self) -> [String; 2] {
        [
            trial_key(TrialScope::Global, &self.name),
            deployment_trial_key(&self.record.deployment_id),
        ]
    }

    pub fn validate(&self) -> Result<(), String> {
        let path = &self.record.skill_dir;
        let valid_hash = matches!(self.record.base_commit.len(), 40 | 64)
            && self
                .record
                .base_commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if self.name.is_empty()
            || self.name.contains(['/', '\\'])
            || matches!(self.name.as_str(), "." | "..")
            || !path.is_absolute()
            || path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
            || path.file_name().and_then(|part| part.to_str()) != Some(self.name.as_str())
            || path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|part| part.to_str())
                != Some("skills")
            || path
                .parent()
                .and_then(|parent| parent.parent())
                .and_then(|parent| parent.file_name())
                .and_then(|part| part.to_str())
                != Some(".agents")
            || self.record.deployment_id
                != deployment_id(
                    &self.name,
                    "global",
                    SkillDestination::Universal,
                    "universal",
                    None,
                    path,
                )
            || self.record.origin_source.is_empty()
            || self.record.repo.is_empty()
            || !valid_hash
            || chrono::DateTime::parse_from_rfc3339(&self.record.forked_at).is_err()
            || self
                .trials_before
                .iter()
                .any(|(key, value)| !self.trial_keys().contains(key) || !value.is_object())
        {
            return Err("Invalid Fork registry transition".into());
        }
        Ok(())
    }

    pub fn apply_document(&self, current: &[u8]) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut document = parse_registry(current)?;
        let record = serde_json::to_value(&self.record).map_err(|error| error.to_string())?;
        let fork = entry(&document, "forks", &self.name)?;
        let before = fork.is_none()
            && self.trial_keys().iter().all(|key| {
                entry(&document, "trials", key)
                    .is_ok_and(|value| value == self.trials_before.get(key))
            });
        let after = fork == Some(&record)
            && self
                .trial_keys()
                .iter()
                .all(|key| entry(&document, "trials", key).is_ok_and(|value| value.is_none()));
        if after {
            return Ok(current.into());
        }
        if !before {
            return Err("Selected Fork or trial entries changed".into());
        }
        set_entry(&mut document, "forks", &self.name, Some(record))?;
        for key in self.trial_keys() {
            set_entry(&mut document, "trials", &key, None)?;
        }
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_REGISTRY_BYTES {
            return Err("Fork registry exceeds its limit".into());
        }
        Ok(bytes)
    }
}

fn parse_registry(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_REGISTRY_BYTES {
        return Err("Fork registry exceeds its limit".into());
    }
    let value: Value = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    if !value.is_object() {
        return Err("Fork registry must be an object".into());
    }
    for section in ["forks", "trials"] {
        if value.get(section).is_some_and(|value| !value.is_object()) {
            return Err(format!("{section} must be an object"));
        }
    }
    Ok(value)
}

fn entry<'a>(document: &'a Value, section: &str, key: &str) -> Result<Option<&'a Value>, String> {
    match document.get(section) {
        None => Ok(None),
        Some(value) => value
            .as_object()
            .map(|map| map.get(key))
            .ok_or_else(|| format!("{section} must be an object")),
    }
}

fn set_entry(
    document: &mut Value,
    section: &str,
    key: &str,
    value: Option<Value>,
) -> Result<(), String> {
    if value.is_none() && document.get(section).is_none() {
        return Ok(());
    }
    let map = document
        .as_object_mut()
        .ok_or("Fork registry must be an object")?
        .entry(section)
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or_else(|| format!("{section} must be an object"))?;
    if let Some(value) = value {
        map.insert(key.into(), value);
    } else {
        map.remove(key);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill_fork_registry::OriginTool;
    use std::path::PathBuf;

    fn record() -> ForkRecord {
        let skill_dir = PathBuf::from("/tmp/home/.agents/skills/alpha");
        ForkRecord {
            deployment_id: deployment_id(
                "alpha",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                &skill_dir,
            ),
            skill_dir,
            forked_at: "2026-09-16T00:00:00Z".into(),
            origin_tool: OriginTool::Dotagents,
            origin_source: "owner/repo".into(),
            repo: "owner/repo".into(),
            path: "skills/alpha".into(),
            declared_ref: Some("main".into()),
            base_commit: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
        }
    }

    #[test]
    fn projects_only_the_selected_fork_and_captured_trials() {
        let deployment_key = deployment_trial_key(&record().deployment_id);
        let bytes = serde_json::to_vec(&serde_json::json!({
            "version": 4,
            "future": {"keep": true},
            "forks": {"sibling": {"unknown": "keep"}},
            "trials": {
                "global/alpha": {"selected": 1, "future": true},
                deployment_key.clone(): {"selected": 2},
                "global/sibling": {"keep": true}
            }
        }))
        .unwrap();
        let transition = ForkRegistryTransition::new("alpha".into(), record(), &bytes).unwrap();
        let proposed = transition.apply_document(&bytes).unwrap();
        assert_eq!(transition.apply_document(&proposed).unwrap(), proposed);
        let document: Value = serde_json::from_slice(&proposed).unwrap();
        assert_eq!(document["future"]["keep"], true);
        assert_eq!(document["forks"]["sibling"]["unknown"], "keep");
        assert_eq!(document["trials"]["global/sibling"]["keep"], true);
        assert!(document["trials"].get("global/alpha").is_none());
        assert!(document["trials"].get(&deployment_key).is_none());
    }

    #[test]
    fn refuses_changed_selected_entries_without_touching_siblings() {
        let bytes = br#"{"forks":{},"trials":{"global/alpha":{"selected":1}}}"#;
        let transition = ForkRegistryTransition::new("alpha".into(), record(), bytes).unwrap();
        let changed = br#"{"forks":{},"trials":{"global/alpha":{"selected":2}}}"#;
        assert!(transition.apply_document(changed).is_err());
    }
}
