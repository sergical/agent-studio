//! Selected fork and trial entries only. This byte transition grants no path or CLI authority.
use crate::skill_deployment::{deployment_id, SkillDestination};
use crate::skill_fork_registry::{deployment_trial_key, trial_key, ForkRecord, TrialScope};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

const MAX_REGISTRY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ForkRegistryTransition {
    name: String,
    record: ForkRecord,
    trials_before: BTreeMap<String, Value>,
}

impl ForkRegistryTransition {
    pub(crate) fn validate_before_projection(
        &self,
        before: &crate::skill_fork_registry::ForkRegistry,
    ) -> Result<(), String> {
        self.validate()?;
        if before.forks.contains_key(&self.name) {
            return Err("Fork record already exists in before-state".into());
        }
        for key in self.trial_keys() {
            let saved = self
                .trials_before
                .get(&key)
                .map(|value| {
                    serde_json::from_value::<crate::skill_fork_registry::TrialRecord>(value.clone())
                })
                .transpose()
                .map_err(|error| error.to_string())?;
            if saved.as_ref() != before.trials.get(&key) {
                return Err("Selected trial projection differs from fork before-state".into());
            }
        }
        Ok(())
    }

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
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
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
                .and_then(|p| p.file_name())
                .and_then(|p| p.to_str())
                != Some("skills")
            || path
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .and_then(|p| p.to_str())
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
            return Err("Invalid fork registry transition".into());
        }
        Ok(())
    }

    pub fn apply_document(&self, current: &[u8]) -> Result<Vec<u8>, String> {
        self.replace(current, true)
    }
    pub fn roll_back_document(&self, current: &[u8]) -> Result<Vec<u8>, String> {
        self.replace(current, false)
    }

    fn replace(&self, current: &[u8], apply: bool) -> Result<Vec<u8>, String> {
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
        if (apply && after) || (!apply && before) {
            return Ok(current.into());
        }
        if (apply && !before) || (!apply && !after) {
            return Err("Selected fork or trial entries changed".into());
        }
        set_entry(&mut document, "forks", &self.name, apply.then_some(record))?;
        for key in self.trial_keys() {
            set_entry(
                &mut document,
                "trials",
                &key,
                if apply {
                    None
                } else {
                    self.trials_before.get(&key).cloned()
                },
            )?;
        }
        let bytes = serde_json::to_vec_pretty(&document).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_REGISTRY_BYTES {
            return Err("Fork registry exceeds its limit".into());
        }
        Ok(bytes)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnforkRegistryTransition {
    selection: ForkRegistryTransition,
    /// V2 binds a historical raw registry row to a freshly admitted canonical
    /// deployment.  It is absent from V1 records, so their persisted bytes keep
    /// the original shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bound: Option<UnforkCurrentBinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UnforkCurrentBinding {
    raw_record: Value,
    parsed_record: ForkRecord,
    deployment_id: String,
    skill_dir: std::path::PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnforkRegistryState {
    Before,
    After,
    Diverged,
}

impl UnforkRegistryTransition {
    pub fn new(name: String, record: ForkRecord, registry: &[u8]) -> Result<Self, String> {
        let document = parse_registry(registry)?;
        let mut selection = ForkRegistryTransition {
            name,
            record,
            trials_before: BTreeMap::new(),
        };
        selection.validate()?;
        if entry(&document, "forks", &selection.name)?
            != Some(&serde_json::to_value(&selection.record).map_err(|error| error.to_string())?)
        {
            return Err("Selected fork differs from the expected Unfork owner".into());
        }
        for key in selection.trial_keys() {
            if let Some(value) = entry(&document, "trials", &key)? {
                selection.trials_before.insert(key, value.clone());
            }
        }
        selection.validate()?;
        Ok(Self {
            selection,
            bound: None,
        })
    }

    /// Bind one saved Fork row to the canonical deployment observed under a
    /// fresh write lease.  Legacy rows may omit their deployment id and path;
    /// populated values must still identify this exact deployment.
    pub fn bind_current(
        name: String,
        deployment_id: String,
        skill_dir: std::path::PathBuf,
        registry: &[u8],
    ) -> Result<Self, String> {
        Self::bind_current_for_origin(
            name,
            deployment_id,
            skill_dir,
            registry,
            crate::skill_fork_registry::OriginTool::Dotagents,
        )
    }

    /// Bind a persisted row to the fresh deployment only when it retains the
    /// provider origin that admitted the operation.
    pub fn bind_current_for_origin(
        name: String,
        deployment_id: String,
        skill_dir: std::path::PathBuf,
        registry: &[u8],
        origin: crate::skill_fork_registry::OriginTool,
    ) -> Result<Self, String> {
        let document = parse_registry(registry)?;
        let raw_record = entry(&document, "forks", &name)?
            .cloned()
            .ok_or("Unfork owner record is missing")?;
        let parsed_record: ForkRecord =
            serde_json::from_value(raw_record.clone()).map_err(|error| error.to_string())?;
        let raw_skill_dir_empty = raw_record
            .get("skill_dir")
            .is_none_or(|value| value.as_str() == Some(""));
        if parsed_record.origin_tool != origin
            || (!parsed_record.deployment_id.is_empty()
                && parsed_record.deployment_id != deployment_id)
            || (!raw_skill_dir_empty
                && !parsed_record.skill_dir.as_os_str().is_empty()
                && parsed_record.skill_dir != skill_dir)
        {
            return Err("Unfork owner record does not match the current deployment".into());
        }
        let mut current = parsed_record.clone();
        current.deployment_id.clone_from(&deployment_id);
        current.skill_dir.clone_from(&skill_dir);
        let mut selection = ForkRegistryTransition {
            name,
            record: current,
            trials_before: BTreeMap::new(),
        };
        selection.validate()?;
        for key in selection.trial_keys() {
            if let Some(value) = entry(&document, "trials", &key)? {
                selection.trials_before.insert(key, value.clone());
            }
        }
        selection.validate()?;
        Ok(Self {
            selection,
            bound: Some(UnforkCurrentBinding {
                raw_record,
                parsed_record,
                deployment_id,
                skill_dir,
            }),
        })
    }

    pub fn record(&self) -> &ForkRecord {
        self.selection.record()
    }
    pub fn name(&self) -> &str {
        &self.selection.name
    }
    pub fn is_bound_current(&self) -> bool {
        self.bound.is_some()
    }
    pub fn recorded_provenance(&self) -> &ForkRecord {
        self.bound
            .as_ref()
            .map(|bound| &bound.parsed_record)
            .unwrap_or(&self.selection.record)
    }
    pub fn validate(&self) -> Result<(), String> {
        self.selection.validate()?;
        if let Some(bound) = &self.bound {
            let parsed: ForkRecord = serde_json::from_value(bound.raw_record.clone())
                .map_err(|error| error.to_string())?;
            let mut expected = parsed.clone();
            expected.deployment_id.clone_from(&bound.deployment_id);
            expected.skill_dir.clone_from(&bound.skill_dir);
            if parsed != bound.parsed_record
                || bound.deployment_id != self.selection.record.deployment_id
                || bound.skill_dir != self.selection.record.skill_dir
                || (!bound.parsed_record.deployment_id.is_empty()
                    && bound.parsed_record.deployment_id != bound.deployment_id)
                || (bound
                    .raw_record
                    .get("skill_dir")
                    .is_some_and(|value| value.as_str() != Some(""))
                    && !bound.parsed_record.skill_dir.as_os_str().is_empty()
                    && bound.parsed_record.skill_dir != bound.skill_dir)
                || expected != self.selection.record
            {
                return Err("Invalid bound Unfork registry transition".into());
            }
        }
        Ok(())
    }

    pub fn observe_document(&self, current: &[u8]) -> Result<UnforkRegistryState, String> {
        self.validate()?;
        self.state(&parse_registry(current)?)
    }

    fn state(&self, current: &Value) -> Result<UnforkRegistryState, String> {
        let selected = &self.selection;
        let record = self
            .bound
            .as_ref()
            .map(|bound| &bound.raw_record)
            .cloned()
            .unwrap_or(serde_json::to_value(&selected.record).map_err(|error| error.to_string())?);
        let fork = entry(current, "forks", &selected.name)?;
        let before = fork == Some(&record)
            && selected.trial_keys().iter().all(|key| {
                entry(current, "trials", key)
                    .is_ok_and(|value| value == selected.trials_before.get(key))
            });
        let after = fork.is_none()
            && selected
                .trial_keys()
                .iter()
                .all(|key| entry(current, "trials", key).is_ok_and(|value| value.is_none()));
        Ok(if before {
            UnforkRegistryState::Before
        } else if after {
            UnforkRegistryState::After
        } else {
            UnforkRegistryState::Diverged
        })
    }

    pub fn apply_document(&self, current: &[u8]) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut document = parse_registry(current)?;
        match self.state(&document)? {
            UnforkRegistryState::After => return Ok(current.into()),
            UnforkRegistryState::Diverged => {
                return Err("Selected fork or trial entries changed before Unfork".into())
            }
            UnforkRegistryState::Before => {}
        }
        set_entry(&mut document, "forks", &self.selection.name, None)?;
        for key in self.selection.trial_keys() {
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
    let root = document
        .as_object_mut()
        .ok_or("Registry is not an object")?;
    let map = root
        .entry(section)
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .ok_or("Registry section is not an object")?;
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
    fn record() -> ForkRecord {
        let path = std::path::PathBuf::from("/fixture/.agents/skills/alpha");
        ForkRecord {
            deployment_id: deployment_id(
                "alpha",
                "global",
                SkillDestination::Universal,
                "universal",
                None,
                &path,
            ),
            skill_dir: path,
            forked_at: "2026-09-11T00:00:00Z".into(),
            origin_tool: OriginTool::SkillsSh,
            origin_source: "owner/repo".into(),
            repo: "owner/repo".into(),
            path: "skills/alpha".into(),
            declared_ref: None,
            base_commit: "a".repeat(40),
        }
    }
    #[test]
    fn unfork_removes_only_selected_owner_and_current_trials_and_retries_identically() {
        let record = record();
        let key = deployment_trial_key(&record.deployment_id);
        let mut document = serde_json::json!({
            "forks": { "alpha": record, "other": { "keep": true } },
            "trials": { "global/alpha": { "current": "legacy key" }, (key.clone()): { "current": "deployment key" }, "project/alpha": { "keep": true } },
            "future": { "keep": true }
        });
        let transition = UnforkRegistryTransition::new(
            "alpha".into(),
            record,
            &serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
        let encoded = serde_json::to_vec(&transition).unwrap();
        assert!(serde_json::from_slice::<Value>(&encoded)
            .unwrap()
            .get("bound")
            .is_none());
        let transition: UnforkRegistryTransition = serde_json::from_slice(&encoded).unwrap();
        document["forks"]["parallel"] = serde_json::json!({"added": true});
        document["future"]["new"] = serde_json::json!(true);
        let before = serde_json::to_vec(&document).unwrap();
        assert_eq!(
            transition.observe_document(&before).unwrap(),
            UnforkRegistryState::Before
        );
        let after = transition.apply_document(&before).unwrap();
        assert_eq!(
            transition.observe_document(&after).unwrap(),
            UnforkRegistryState::After
        );
        assert_eq!(transition.apply_document(&after).unwrap(), after);
        let actual: Value = serde_json::from_slice(&after).unwrap();
        let mut expected = document;
        expected["forks"].as_object_mut().unwrap().remove("alpha");
        expected["trials"]
            .as_object_mut()
            .unwrap()
            .remove("global/alpha");
        expected["trials"].as_object_mut().unwrap().remove(&key);
        assert_eq!(actual, expected);
        assert_eq!(transition.name(), "alpha");
    }

    #[test]
    fn bound_current_removes_a_legacy_raw_owner_without_normalizing_it() {
        let record = record();
        let mut document = serde_json::json!({
            "forks": { "alpha": record, "other": { "keep": true } },
            "trials": { "global/alpha": { "keep": true }, "project/alpha": { "other": true } },
            "unknown": { "keep": true }
        });
        document["forks"]["alpha"]
            .as_object_mut()
            .unwrap()
            .remove("deployment_id");
        document["forks"]["alpha"]
            .as_object_mut()
            .unwrap()
            .remove("skill_dir");
        document["forks"]["alpha"]["origin_tool"] = serde_json::json!("dotagents");
        document["forks"]["alpha"]["future"] = serde_json::json!({"keep": true});
        let transition = UnforkRegistryTransition::bind_current(
            "alpha".into(),
            record.deployment_id.clone(),
            record.skill_dir.clone(),
            &serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
        let after: Value = serde_json::from_slice(
            &transition
                .apply_document(&serde_json::to_vec(&document).unwrap())
                .unwrap(),
        )
        .unwrap();
        assert!(after["forks"].get("alpha").is_none());
        assert_eq!(after["forks"]["other"]["keep"], true);
        assert_eq!(after["trials"]["project/alpha"]["other"], true);
        assert_eq!(after["unknown"]["keep"], true);
    }

    #[test]
    fn bound_current_refuses_raw_drift_and_wrong_canonical_identity() {
        let record = record();
        let mut document = serde_json::json!({ "forks": { "alpha": record } });
        document["forks"]["alpha"]
            .as_object_mut()
            .unwrap()
            .remove("deployment_id");
        document["forks"]["alpha"]
            .as_object_mut()
            .unwrap()
            .remove("skill_dir");
        document["forks"]["alpha"]["origin_tool"] = serde_json::json!("dotagents");
        let transition = UnforkRegistryTransition::bind_current(
            "alpha".into(),
            record.deployment_id.clone(),
            record.skill_dir.clone(),
            &serde_json::to_vec(&document).unwrap(),
        )
        .unwrap();
        let mut changed = document.clone();
        changed["forks"]["alpha"]["origin_source"] = serde_json::json!("other/repo");
        assert!(transition
            .apply_document(&serde_json::to_vec(&changed).unwrap())
            .is_err());
        assert!(UnforkRegistryTransition::bind_current(
            "alpha".into(),
            "wrong".into(),
            record.skill_dir,
            &serde_json::to_vec(&document).unwrap(),
        )
        .is_err());
        let mut encoded = serde_json::to_value(&transition).unwrap();
        encoded["selection"]["record"]["origin_source"] = serde_json::json!("other/repo");
        assert!(serde_json::from_value::<UnforkRegistryTransition>(encoded)
            .unwrap()
            .validate()
            .is_err());
    }

    #[test]
    fn unfork_refuses_changed_selected_records_and_partial_or_invalid_state() {
        let record = record();
        let original = serde_json::json!({"forks":{"alpha":record},"trials":{"global/alpha":{"current":true}}});
        let bytes = serde_json::to_vec(&original).unwrap();
        let transition =
            UnforkRegistryTransition::new("alpha".into(), record.clone(), &bytes).unwrap();
        for change in [
            "fork",
            "trial",
            "missing-trial",
            "missing-fork",
            "unknown-owner-field",
        ] {
            let mut current = original.clone();
            match change {
                "fork" => {
                    current["forks"]["alpha"]["base_commit"] = serde_json::json!("b".repeat(40))
                }
                "trial" => current["trials"]["global/alpha"] = serde_json::json!({"new":true}),
                "missing-trial" => {
                    current["trials"]
                        .as_object_mut()
                        .unwrap()
                        .remove("global/alpha");
                }
                "missing-fork" => {
                    current["forks"].as_object_mut().unwrap().remove("alpha");
                }
                "unknown-owner-field" => {
                    current["forks"]["alpha"]["future"] = serde_json::json!(true)
                }
                _ => unreachable!(),
            }
            let current = serde_json::to_vec(&current).unwrap();
            assert_eq!(
                transition.observe_document(&current).unwrap(),
                UnforkRegistryState::Diverged,
                "{change}"
            );
            assert!(transition.apply_document(&current).is_err(), "{change}");
        }
        assert!(UnforkRegistryTransition::new("alpha".into(), record, b"{}").is_err());
        for invalid in [b"[]".as_slice(), br#"{"trials":null}"#.as_slice()] {
            assert!(transition.observe_document(invalid).is_err());
            assert!(transition.apply_document(invalid).is_err());
        }
        assert!(transition
            .apply_document(&vec![b' '; MAX_REGISTRY_BYTES + 1])
            .is_err());
        let mut serialized = serde_json::to_value(&transition).unwrap();
        serialized["selection"]["trials_before"]["global/other"] = serde_json::json!({"keep":true});
        let expanded: UnforkRegistryTransition = serde_json::from_value(serialized).unwrap();
        assert!(expanded.apply_document(&bytes).is_err());
    }

    #[test]
    fn selected_entries_round_trip_and_preserve_later_preferences() {
        let record = record();
        let key = deployment_trial_key(&record.deployment_id);
        let original = serde_json::json!({"forks":{"other":{"future":"keep"}},"trials":{"global/alpha":{"legacy":true}, (key.clone()):{"unknown":42}, "project/alpha":{"keep":true}},"preference":"before"});
        let bytes = serde_json::to_vec(&original).unwrap();
        let transition = ForkRegistryTransition::new("alpha".into(), record, &bytes).unwrap();
        let encoded = serde_json::to_vec(&transition).unwrap();
        let transition: ForkRegistryTransition = serde_json::from_slice(&encoded).unwrap();
        let applied = transition.apply_document(&bytes).unwrap();
        assert_eq!(transition.apply_document(&applied).unwrap(), applied);
        let mut later: Value = serde_json::from_slice(&applied).unwrap();
        assert!(later["trials"].get("global/alpha").is_none());
        assert!(later["trials"].get(&key).is_none());
        later["preference"] = serde_json::json!("after");
        later["forks"]["new"] = serde_json::json!({"keep":"also"});
        let rolled = transition
            .roll_back_document(&serde_json::to_vec(&later).unwrap())
            .unwrap();
        assert_eq!(transition.roll_back_document(&rolled).unwrap(), rolled);
        let rolled: Value = serde_json::from_slice(&rolled).unwrap();
        assert!(rolled["forks"].get("alpha").is_none());
        assert_eq!(rolled["trials"], original["trials"]);
        assert_eq!(rolled["forks"]["other"], original["forks"]["other"]);
        assert_eq!(rolled["forks"]["new"], later["forks"]["new"]);
        assert_eq!(rolled["preference"], "after");
    }
    #[test]
    fn refuses_changed_selected_entries_and_invalid_shapes() {
        let record = record();
        let original = serde_json::json!({"trials":{"global/alpha":{"old":true}}});
        let bytes = serde_json::to_vec(&original).unwrap();
        let transition =
            ForkRegistryTransition::new("alpha".into(), record.clone(), &bytes).unwrap();
        for current in [
            serde_json::json!({"trials":{"global/alpha":{"new":true}}}),
            serde_json::json!({}),
            serde_json::json!({"forks":{"alpha":record},"trials":{"global/alpha":{"old":true}}}),
            serde_json::json!({"trials":null}),
        ] {
            assert!(transition
                .apply_document(&serde_json::to_vec(&current).unwrap())
                .is_err());
        }
        let applied = transition.apply_document(&bytes).unwrap();
        let mut changed: Value = serde_json::from_slice(&applied).unwrap();
        changed["trials"]["global/alpha"] = serde_json::json!({"replacement":true});
        assert!(transition
            .roll_back_document(&serde_json::to_vec(&changed).unwrap())
            .is_err());
        assert!(ForkRegistryTransition::new("alpha".into(), record.clone(), b"[]").is_err());
        assert!(ForkRegistryTransition::new(
            "alpha".into(),
            record.clone(),
            br#"{"trials":{"global/alpha":null}}"#
        )
        .is_err());
        let mut invalid = record;
        invalid.deployment_id = "wrong".into();
        assert!(ForkRegistryTransition::new("alpha".into(), invalid, b"{}").is_err());
    }
    #[test]
    fn serialized_transition_cannot_expand_selected_entries_or_bypass_budget() {
        let transition = ForkRegistryTransition::new("alpha".into(), record(), b"{}").unwrap();
        for change in ["trial", "name", "commit"] {
            let mut encoded = serde_json::to_value(&transition).unwrap();
            match change {
                "trial" => {
                    encoded["trials_before"]["global/other"] = serde_json::json!({"keep":true})
                }
                "name" => encoded["name"] = serde_json::json!("other"),
                "commit" => encoded["record"]["base_commit"] = serde_json::json!("main"),
                _ => unreachable!(),
            }
            let decoded: ForkRegistryTransition = serde_json::from_value(encoded).unwrap();
            assert!(decoded.apply_document(b"{}").is_err());
        }
        assert!(transition
            .apply_document(&vec![b' '; MAX_REGISTRY_BYTES + 1])
            .is_err());
        let near_limit =
            serde_json::to_vec(&serde_json::json!({"padding":"x".repeat(MAX_REGISTRY_BYTES - 32)}))
                .unwrap();
        assert!(near_limit.len() <= MAX_REGISTRY_BYTES);
        assert!(transition.apply_document(&near_limit).is_err());
    }
}
