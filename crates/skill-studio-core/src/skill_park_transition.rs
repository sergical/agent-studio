//! Exact, raw JSON registry publication for Park and Unpark.
//!
//! This deliberately edits only the selected parked record and one exact
//! trial.  The registry is user data as well as Skill Studio data, so a typed
//! deserialize/serialize round trip is not a safe publication mechanism.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParkRegistryTransition {
    name: String,
    before_parked: Option<Value>,
    after_parked: Option<Value>,
    trial: Option<TrialTransition>,
    owner: Option<OwnerTransition>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrialTransition {
    before_key: String,
    before: Value,
    after_key: String,
    after: Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OwnerTransition {
    section: String,
    before_key: String,
    before: Option<Value>,
    after_key: String,
    after: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParkRegistryState {
    Before,
    After,
    Conflict,
}

impl ParkRegistryTransition {
    pub fn park(
        name: impl Into<String>,
        parked: Value,
        trial: Option<TrialTransition>,
    ) -> Result<Self, String> {
        let transition = Self {
            name: name.into(),
            before_parked: None,
            after_parked: Some(parked),
            trial,
            owner: None,
        };
        transition.validate()?;
        Ok(transition)
    }

    pub fn unpark(
        name: impl Into<String>,
        parked: Value,
        trial: Option<TrialTransition>,
    ) -> Result<Self, String> {
        let transition = Self {
            name: name.into(),
            before_parked: Some(parked),
            after_parked: None,
            trial,
            owner: None,
        };
        transition.validate()?;
        Ok(transition)
    }

    pub fn trial(
        before_key: impl Into<String>,
        before: Value,
        after_key: impl Into<String>,
        after: Value,
    ) -> TrialTransition {
        TrialTransition {
            before_key: before_key.into(),
            before,
            after_key: after_key.into(),
            after,
        }
    }

    pub fn with_owner(
        mut self,
        section: impl Into<String>,
        before_key: impl Into<String>,
        before: Option<Value>,
        after_key: impl Into<String>,
        after: Option<Value>,
    ) -> Result<Self, String> {
        self.owner = Some(OwnerTransition {
            section: section.into(),
            before_key: before_key.into(),
            before,
            after_key: after_key.into(),
            after,
        });
        self.validate()?;
        Ok(self)
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn state(&self, bytes: &[u8]) -> Result<ParkRegistryState, String> {
        self.validate()?;
        let root: Value = if bytes.is_empty() {
            Value::Object(Map::new())
        } else {
            strict_document(bytes)?
        };
        let object = root.as_object().ok_or("Registry root must be an object")?;
        let section = |name: &str| -> Result<Option<&Map<String, Value>>, String> {
            object
                .get(name)
                .map(|value| {
                    value
                        .as_object()
                        .ok_or_else(|| format!("Registry {name} field must be an object"))
                })
                .transpose()
        };
        let matches = |after: bool| -> Result<bool, String> {
            let parked = section("parked")?;
            let expected_parked = if after {
                self.after_parked.as_ref()
            } else {
                self.before_parked.as_ref()
            };
            if parked.and_then(|records| records.get(&self.name)) != expected_parked {
                return Ok(false);
            }
            if let Some(trial) = &self.trial {
                let trials = section("trials")?;
                let (key, expected, absent_key) = if after {
                    (
                        &trial.after_key,
                        &trial.after,
                        (trial.before_key != trial.after_key).then_some(&trial.before_key),
                    )
                } else {
                    (
                        &trial.before_key,
                        &trial.before,
                        (trial.before_key != trial.after_key).then_some(&trial.after_key),
                    )
                };
                if trials.and_then(|records| records.get(key)) != Some(expected)
                    || absent_key
                        .is_some_and(|key| trials.is_some_and(|records| records.contains_key(key)))
                {
                    return Ok(false);
                }
            }
            if let Some(owner) = &self.owner {
                let records = section(&owner.section)?;
                let (key, expected, absent_key) = if after {
                    (
                        &owner.after_key,
                        owner.after.as_ref(),
                        (owner.before_key != owner.after_key).then_some(&owner.before_key),
                    )
                } else {
                    (
                        &owner.before_key,
                        owner.before.as_ref(),
                        (owner.before_key != owner.after_key).then_some(&owner.after_key),
                    )
                };
                if records.and_then(|values| values.get(key)) != expected
                    || absent_key
                        .is_some_and(|key| records.is_some_and(|values| values.contains_key(key)))
                {
                    return Ok(false);
                }
            }
            Ok(true)
        };
        if matches(false)? {
            Ok(ParkRegistryState::Before)
        } else if matches(true)? {
            Ok(ParkRegistryState::After)
        } else {
            Ok(ParkRegistryState::Conflict)
        }
    }

    pub fn apply(&self, bytes: &[u8]) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut root: Value = if bytes.is_empty() {
            Value::Object(Map::new())
        } else {
            strict_document(bytes)?
        };
        let object = root
            .as_object_mut()
            .ok_or("Registry root must be an object")?;
        let parked = object
            .entry("parked")
            .or_insert_with(|| Value::Object(Map::new()));
        let parked = parked
            .as_object_mut()
            .ok_or("Registry parked field must be an object")?;
        if parked.get(&self.name) != self.before_parked.as_ref() {
            return Err("Park registry record changed before publication".into());
        }
        match &self.after_parked {
            Some(value) => {
                parked.insert(self.name.clone(), value.clone());
            }
            None => {
                parked.remove(&self.name);
            }
        }
        if let Some(trial) = &self.trial {
            let trials = object
                .entry("trials")
                .or_insert_with(|| Value::Object(Map::new()));
            let trials = trials
                .as_object_mut()
                .ok_or("Registry trials field must be an object")?;
            if trials.get(&trial.before_key) != Some(&trial.before) {
                return Err("Park trial changed before publication".into());
            }
            if trial.before_key != trial.after_key && trials.contains_key(&trial.after_key) {
                return Err("Park trial destination is occupied".into());
            }
            trials.remove(&trial.before_key);
            trials.insert(trial.after_key.clone(), trial.after.clone());
        }
        if let Some(owner) = &self.owner {
            if !matches!(owner.section.as_str(), "copies" | "forks") {
                return Err("Invalid Park owner section".into());
            }
            let section = object
                .entry(&owner.section)
                .or_insert_with(|| Value::Object(Map::new()));
            let section = section
                .as_object_mut()
                .ok_or("Registry owner section must be an object")?;
            if section.get(&owner.before_key) != owner.before.as_ref() {
                return Err("Park owner changed before publication".into());
            }
            if owner.before_key != owner.after_key
                && owner.after.is_some()
                && section.contains_key(&owner.after_key)
            {
                return Err("Park owner destination is occupied".into());
            }
            section.remove(&owner.before_key);
            if let Some(value) = &owner.after {
                section.insert(owner.after_key.clone(), value.clone());
            }
        }
        serde_json::to_vec_pretty(&root).map_err(|e| e.to_string())
    }

    fn validate(&self) -> Result<(), String> {
        if self.name.is_empty()
            || matches!(self.name.as_str(), "." | "..")
            || self.name.contains('/')
            || self.name.contains('\\')
        {
            return Err("Invalid Park skill name".into());
        }
        if self.before_parked.is_none() == self.after_parked.is_none() {
            return Err("Park transition must add or remove one parked record".into());
        }
        if let Some(owner) = &self.owner {
            if !matches!(owner.section.as_str(), "copies" | "forks")
                || owner.before_key.is_empty()
                || owner.after_key.is_empty()
                || (owner.before.is_none() && owner.after.is_none())
            {
                return Err("Invalid Park owner transition".into());
            }
        }
        Ok(())
    }
}

fn strict_document(bytes: &[u8]) -> Result<Value, String> {
    serde_json::from_slice::<crate::skill_skills_sh_lock_transition::UniqueJson>(bytes)
        .map(|document| document.0)
        .map_err(|error| format!("Registry is malformed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn preserves_unknown_and_unrelated_registry_fields() {
        let before = br#"{"future":{"keep":true},"parked":{},"trials":{"other":{"keep":1}}}"#;
        let transition = ParkRegistryTransition::park(
            "skill",
            json!({"source_kind":"manual","unknown":"kept"}),
            None,
        )
        .unwrap();
        let after: Value = serde_json::from_slice(&transition.apply(before).unwrap()).unwrap();
        assert_eq!(after["future"]["keep"], true);
        assert_eq!(after["trials"]["other"]["keep"], 1);
        assert_eq!(after["parked"]["skill"]["unknown"], "kept");
    }

    #[test]
    fn refuses_a_changed_selected_record() {
        let transition =
            ParkRegistryTransition::unpark("skill", json!({"parked_at":"old"}), None).unwrap();
        assert!(transition
            .apply(br#"{"parked":{"skill":{"parked_at":"new"}}}"#)
            .is_err());
    }

    #[test]
    fn classifies_only_exact_selected_before_and_after_states() {
        let transition = ParkRegistryTransition::park(
            "skill",
            json!({"parked_at":"now"}),
            Some(ParkRegistryTransition::trial(
                "before",
                json!({"path":"live","future":true}),
                "after",
                json!({"path":"parked","future":true}),
            )),
        )
        .unwrap()
        .with_owner(
            "copies",
            "copy",
            Some(json!({"path":"live","future":true})),
            "copy",
            None,
        )
        .unwrap();
        let before = br#"{"parked":{},"trials":{"before":{"path":"live","future":true}},"copies":{"copy":{"path":"live","future":true}},"unknown":1}"#;
        assert_eq!(transition.state(before).unwrap(), ParkRegistryState::Before);
        let after = transition.apply(before).unwrap();
        assert_eq!(transition.state(&after).unwrap(), ParkRegistryState::After);
        let mut drift: Value = serde_json::from_slice(&after).unwrap();
        drift["parked"]["skill"]["parked_at"] = json!("changed");
        assert_eq!(
            transition
                .state(&serde_json::to_vec(&drift).unwrap())
                .unwrap(),
            ParkRegistryState::Conflict
        );
    }

    #[test]
    fn rejects_wrong_typed_sections_and_duplicate_keys() {
        let transition =
            ParkRegistryTransition::park("skill", json!({"parked_at":"now"}), None).unwrap();
        for malformed in [
            br#"{"parked":[]}"#.as_slice(),
            br#"{"parked":{},"parked":{"skill":{"parked_at":"now"}}}"#.as_slice(),
        ] {
            assert!(transition.state(malformed).is_err());
            assert!(transition.apply(malformed).is_err());
        }
    }
}
