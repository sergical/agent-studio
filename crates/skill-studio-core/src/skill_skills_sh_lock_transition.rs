//! Strict selected-row transitions for the global skills.sh v3 lock.
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub(crate) const MAX_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

pub(crate) struct UniqueJson(pub(crate) Value);

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl<'de> serde::de::Visitor<'de> for Visitor {
            type Value = UniqueJson;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("JSON with unique object keys")
            }
            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::Bool(value)))
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(value.into()))
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<UniqueJson, E> {
                Ok(UniqueJson(value.into()))
            }
            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<UniqueJson, E> {
                serde_json::Number::from_f64(value)
                    .map(|number| UniqueJson(Value::Number(number)))
                    .ok_or_else(|| E::custom("Non-finite JSON number"))
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::String(value.into())))
            }
            fn visit_unit<E: serde::de::Error>(self) -> Result<UniqueJson, E> {
                Ok(UniqueJson(Value::Null))
            }
            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut values = Vec::new();
                while let Some(UniqueJson(value)) = sequence.next_element()? {
                    values.push(value);
                }
                Ok(UniqueJson(Value::Array(values)))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<UniqueJson, A::Error> {
                let mut values = serde_json::Map::new();
                while let Some((key, UniqueJson(value))) = map.next_entry::<String, UniqueJson>()? {
                    if values.insert(key, value).is_some() {
                        return Err(serde::de::Error::custom("Duplicate JSON object key"));
                    }
                }
                Ok(UniqueJson(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

pub(crate) fn json_document(bytes: &[u8]) -> Result<Value, String> {
    if bytes.len() > MAX_DOCUMENT_BYTES {
        return Err("skills.sh lock exceeds its size limit".into());
    }
    serde_json::from_slice::<UniqueJson>(bytes)
        .map(|document| document.0)
        .map_err(|error| format!("Invalid skills.sh lock: {error}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillsShLockState {
    Detached,
    Attached,
    Diverged,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsShLockTransition {
    name: String,
    selected: Value,
}

impl SkillsShLockTransition {
    pub fn read(
        lock: &[u8],
        name: &str,
        repo: &str,
        path: &str,
        declared_ref: Option<&str>,
    ) -> Result<Self, String> {
        let document = json_document(lock)?;
        let selected = skills(&document)?
            .get(name)
            .cloned()
            .ok_or("Selected skills.sh lock row is missing")?;
        let row = selected
            .as_object()
            .ok_or("Selected skills.sh lock row is invalid")?;
        if row.get("source").and_then(Value::as_str) != Some(repo)
            || row.get("sourceType").and_then(Value::as_str) != Some("github")
            || row.get("sourceUrl").and_then(Value::as_str)
                != Some(&format!("https://github.com/{repo}.git"))
            || row.get("skillPath").and_then(Value::as_str)
                != Some(&if path.is_empty() {
                    "SKILL.md".into()
                } else {
                    format!("{path}/SKILL.md")
                })
            || !row
                .get("skillFolderHash")
                .and_then(Value::as_str)
                .is_some_and(|hash| {
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                })
            || row
                .get("installedAt")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            || row
                .get("updatedAt")
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        {
            return Err("Selected skills.sh lock row differs from the admitted source".into());
        }
        match (declared_ref, row.get("ref")) {
            (None, None) => {}
            (Some(expected), Some(Value::String(actual))) if actual == expected => {}
            _ => return Err("Selected skills.sh lock row has an invalid ref".into()),
        }
        Ok(Self {
            name: name.into(),
            selected,
        })
    }

    pub fn observe(&self, current: &[u8]) -> Result<SkillsShLockState, String> {
        let document = json_document(current)?;
        let current = skills(&document)?.get(&self.name);
        Ok(match current {
            None => SkillsShLockState::Detached,
            Some(value) if value == &self.selected => SkillsShLockState::Attached,
            Some(_) => SkillsShLockState::Diverged,
        })
    }

    pub fn require_only_selected(lock: &[u8], name: &str) -> Result<(), String> {
        let document = json_document(lock)?;
        let entries = skills(&document)?;
        if entries.len() != 1 || !entries.contains_key(name) {
            return Err("Staged skills.sh lock must contain only the selected skill".into());
        }
        Ok(())
    }

    pub fn apply_document(&self, current: &[u8]) -> Result<Vec<u8>, String> {
        let mut document = json_document(current)?;
        match self.observe(current)? {
            SkillsShLockState::Attached => return Ok(current.into()),
            SkillsShLockState::Diverged => return Err("Selected skills.sh lock row changed".into()),
            SkillsShLockState::Detached => {}
        }
        skills_mut(&mut document)?.insert(self.name.clone(), self.selected.clone());
        serde_json::to_vec(&document).map_err(|error| error.to_string())
    }
}

fn skills(document: &Value) -> Result<&serde_json::Map<String, Value>, String> {
    if document.get("version").and_then(Value::as_u64) != Some(3) {
        return Err("Unfork requires a version 3 skills.sh lock".into());
    }
    document
        .get("skills")
        .and_then(Value::as_object)
        .ok_or("skills.sh lock has no skills object".into())
}

fn skills_mut(document: &mut Value) -> Result<&mut serde_json::Map<String, Value>, String> {
    if document.get("version").and_then(Value::as_u64) != Some(3) {
        return Err("Unfork requires a version 3 skills.sh lock".into());
    }
    document
        .as_object_mut()
        .and_then(|root| root.get_mut("skills"))
        .and_then(Value::as_object_mut)
        .ok_or("skills.sh lock has no skills object".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn row() -> Vec<u8> {
        br#"{"version":3,"unknown":true,"skills":{"alpha":{"source":"owner/repo","sourceType":"github","sourceUrl":"https://github.com/owner/repo.git","skillPath":"skills/alpha/SKILL.md","skillFolderHash":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","installedAt":"x","updatedAt":"y","unknown":"kept"},"other":{"kept":true}}}"#.to_vec()
    }
    #[test]
    fn projects_only_the_selected_row() {
        let transition =
            SkillsShLockTransition::read(&row(), "alpha", "owner/repo", "skills/alpha", None)
                .unwrap();
        let live = br#"{"version":3,"root":"kept","skills":{"other":{"kept":true}}}"#;
        let applied = transition.apply_document(live).unwrap();
        assert_eq!(
            transition.observe(&applied).unwrap(),
            SkillsShLockState::Attached
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&applied).unwrap()["root"],
            "kept"
        );
    }

    #[test]
    fn rejects_missing_required_row_fields_and_preserves_unknown_values() {
        let mut invalid = serde_json::from_slice::<Value>(&row()).unwrap();
        invalid["skills"]["alpha"]["ref"] = Value::String("main".into());
        assert!(SkillsShLockTransition::read(
            &serde_json::to_vec(&invalid).unwrap(),
            "alpha",
            "owner/repo",
            "skills/alpha",
            None,
        )
        .is_err());
        let transition =
            SkillsShLockTransition::read(&row(), "alpha", "owner/repo", "skills/alpha", None)
                .unwrap();
        let applied = transition
            .apply_document(
                br#"{"version":3,"future":{"keep":true},"skills":{"other":{"future":7}}}"#,
            )
            .unwrap();
        let value: Value = serde_json::from_slice(&applied).unwrap();
        assert_eq!(value["future"]["keep"], true);
        assert_eq!(value["skills"]["other"]["future"], 7);
        assert_eq!(value["skills"]["alpha"]["unknown"], "kept");
    }
}
