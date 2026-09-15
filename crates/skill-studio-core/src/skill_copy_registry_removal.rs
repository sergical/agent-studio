use crate::{skill_copy_removal::CopyRemovalReader, skill_fork_registry::CopyDeploymentRecord};
use std::{collections::BTreeMap, path::Path};

/// Computes selected record removal from the current registry document.
/// The caller validates its durable intent and publishes these bytes with CAS.
pub(crate) fn remove_copy_records(
    original: &[u8],
    selected: &CopyDeploymentRecord,
    readers: &[CopyRemovalReader],
    expected_copies: &BTreeMap<String, serde_json::Value>,
    expected_trials: &BTreeMap<String, serde_json::Value>,
) -> Result<Vec<u8>, String> {
    if original.len() > 8 * 1024 * 1024 {
        return Err("Copy registry exceeds its limit".into());
    }
    let mut document: serde_json::Value =
        serde_json::from_slice(original).map_err(|e| e.to_string())?;
    let trial_snapshot = document.get("trials").cloned();
    let copies_snapshot = document
        .get("copies")
        .and_then(serde_json::Value::as_object)
        .ok_or("Copy registry records are missing or invalid")?;
    let selected_present = copies_snapshot.contains_key(&selected.deployment_id);
    let every_removed = expected_copies
        .keys()
        .all(|key| !copies_snapshot.contains_key(key))
        && expected_trials
            .keys()
            .all(|key| trial_snapshot.as_ref().and_then(|v| v.get(key)).is_none());
    if !selected_present && every_removed {
        return Ok(original.to_vec());
    }
    for reader in readers {
        match (
            &reader.registry_value,
            copies_snapshot.get(&reader.deployment_id),
        ) {
            (Some(expected), Some(actual)) if expected == actual => {}
            (None, None) => {}
            _ => return Err("Copy reader ownership changed".into()),
        }
    }
    if let Some(trials) = trial_snapshot
        .as_ref()
        .and_then(serde_json::Value::as_object)
    {
        for (key, value) in trials {
            if raw_trial_relates(value, selected, readers)
                && expected_trials.get(key) != Some(value)
            {
                return Err("Copy trial ownership changed".into());
            }
        }
    }
    for (key, expected) in expected_copies {
        if copies_snapshot.get(key) != Some(expected) {
            return Err("Copy ownership changed or removal is partially published".into());
        }
    }
    for (key, expected) in expected_trials {
        if trial_snapshot.as_ref().and_then(|trials| trials.get(key)) != Some(expected) {
            return Err("Copy trial ownership changed or removal is partially published".into());
        }
    }
    let root = document
        .as_object_mut()
        .ok_or("Copy registry is not an object")?;
    let copies = root
        .get_mut("copies")
        .and_then(serde_json::Value::as_object_mut)
        .ok_or("Copy registry records are missing or invalid")?;
    for key in expected_copies.keys() {
        copies.remove(key);
    }
    let trials = root
        .get_mut("trials")
        .and_then(serde_json::Value::as_object_mut);
    if let Some(trials) = trials {
        for key in expected_trials.keys() {
            trials.remove(key);
        }
    }
    serde_json::to_vec(&document).map_err(|e| e.to_string())
}

pub(crate) fn raw_trial_relates(
    value: &serde_json::Value,
    selected: &CopyDeploymentRecord,
    readers: &[CopyRemovalReader],
) -> bool {
    let ids = std::iter::once(selected.deployment_id.as_str())
        .chain(readers.iter().map(|reader| reader.deployment_id.as_str()));
    let paths = std::iter::once(selected.path.as_path())
        .chain(readers.iter().map(|reader| reader.path.as_path()));
    value
        .get("deployment_id")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|id| ids.clone().any(|candidate| candidate == id))
        || value
            .get("skill_dir")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|path| paths.clone().any(|candidate| candidate == Path::new(path)))
        || value
            .get("claude_link")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|path| readers.iter().any(|reader| reader.path == Path::new(path)))
}
