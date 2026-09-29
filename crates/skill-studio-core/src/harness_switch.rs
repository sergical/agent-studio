//! Content transforms for each harness's native per-skill switch.
//!
//! Every function here is a pure string-in, string-out transform: it never
//! touches a filesystem. [`crate::ops::set_harness_enabled`] reads the
//! current bytes through [`crate::ports::ScopeFs`], calls the transform for
//! the target harness, and writes the result back - the same split
//! `codex_skill_config.rs`/`opencode_skill_permission.rs` drew on the
//! desktop, kept here so the transform itself is testable with plain
//! strings and the core stays free of `std::fs` (see `docs/action-map/
//! definition-of-done.md`'s primitive checklist).
//!
//! Codex has no transform here: `crate::ops::set_codex_switch` shares
//! `crate::ops::codex_write_disabled_row`, the decor-preserving
//! `[[skills.config]]` row writer, with `set_codex_skill_disabled_with` rather
//! than duplicating it as a plain-string transform - a second writer for
//! the same file only invites the two to drift.

use serde_json::{Map, Value};

use crate::error::{CoreError, ErrorCode};

/// Cap on a harness config file this module reads, matching the order of
/// magnitude `crate::ops::SKILL_MD_MAX_BYTES` uses for `SKILL.md` - these
/// are hand-maintained config files, not data dumps.
pub(crate) const HARNESS_CONFIG_MAX_BYTES: u64 = 1_048_576;

/// `true` when only `opencode.jsonc` exists: Skill Studio never parses that
/// format, so writing `permission.skill` would either create a `.json`
/// sibling `OpenCode` must then merge, or silently drop the user's comments.
pub(crate) fn opencode_refuses_jsonc(
    json_exists: bool,
    jsonc_exists: bool,
) -> Result<(), CoreError> {
    if jsonc_exists && !json_exists {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "OpenCode's config is opencode.jsonc; edit permission.skill by hand",
        ));
    }
    Ok(())
}

/// Sets (`deny`) or clears `permission.skill.<name>` in `opencode.json`
/// text, given its current text (`None` for a missing file). Ports
/// `opencode_skill_permission.rs::set_skill_denied`'s JSON edit onto a plain
/// string.
pub(crate) fn opencode_toggle(
    existing: Option<&str>,
    name: &str,
    denied: bool,
) -> Result<String, CoreError> {
    let mut root: Map<String, Value> = match existing {
        Some(text) => serde_json::from_str(text).map_err(|e| {
            CoreError::new(
                ErrorCode::Io,
                format!("opencode.json is not valid JSON: {e}"),
            )
        })?,
        None => Map::new(),
    };
    if !root.contains_key("$schema") {
        root.insert(
            "$schema".to_string(),
            Value::String("https://opencode.ai/config.json".to_string()),
        );
    }

    let permission = root
        .entry("permission")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(permission) = permission else {
        return Err(CoreError::new(
            ErrorCode::Io,
            "opencode.json has a non-object `permission` key",
        ));
    };
    let skill = permission
        .entry("skill")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(skill) = skill else {
        return Err(CoreError::new(
            ErrorCode::Io,
            "opencode.json has a non-object `permission.skill` key",
        ));
    };

    if denied {
        skill.insert(name.to_string(), Value::String("deny".to_string()));
    } else {
        skill.remove(name);
        if skill.is_empty() {
            permission.remove("skill");
        }
        if permission.is_empty() {
            root.remove("permission");
        }
    }

    serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| {
        CoreError::new(
            ErrorCode::Io,
            format!("failed to serialize opencode.json: {e}"),
        )
    })
}

/// The `skillOverrides` value Claude Code reads as "hidden from Claude and
/// the `/` menu".
pub(crate) const CLAUDE_SKILL_OVERRIDE_OFF: &str = "off";

/// Sets `skillOverrides.<name>` in Claude Code's `settings.json` text to
/// `value`, or removes the key when `value` is `None`, and returns the new
/// text with the entry's earlier value. Only that one entry changes: every
/// other `skillOverrides` entry and every other top-level key is kept, in
/// its order. An empty `skillOverrides` object left by a removal is dropped.
/// Invalid JSON, a non-object root, or a non-object `skillOverrides` is
/// refused rather than replaced, because the same file holds the user's
/// permissions, hooks, and plugin settings.
pub(crate) fn claude_skill_override_set(
    existing: Option<&str>,
    name: &str,
    value: Option<Value>,
) -> Result<(String, Option<Value>), CoreError> {
    let mut root: Map<String, Value> = match existing {
        Some(text) => serde_json::from_str(text).map_err(|e| {
            CoreError::new(
                ErrorCode::Io,
                format!("Claude Code settings.json is not a JSON object: {e}"),
            )
        })?,
        None => Map::new(),
    };
    let overrides = root
        .entry("skillOverrides")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(overrides) = overrides else {
        return Err(CoreError::new(
            ErrorCode::Io,
            "Claude Code settings.json has a non-object `skillOverrides` key",
        ));
    };
    let earlier = match value {
        Some(value) => overrides.insert(name.to_string(), value),
        None => overrides.shift_remove(name),
    };
    if overrides.is_empty() {
        root.shift_remove("skillOverrides");
    }
    let text = serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| {
        CoreError::new(
            ErrorCode::Io,
            format!("failed to serialize Claude Code settings.json: {e}"),
        )
    })?;
    Ok((text, earlier))
}

/// pi has no native per-skill switch (`docs/action-map/enable-and-links.md`
/// names none), so this build stands one up: an exclusion list under a
/// `skill-studio` key in pi's own `settings.json`, left alone by pi itself.
/// Adds or removes `name` from `skill-studio.disabledSkills`, given the
/// file's current text (`None` for a missing file).
pub(crate) fn pi_toggle(
    existing: Option<&str>,
    name: &str,
    disabled: bool,
) -> Result<String, CoreError> {
    let mut root: Map<String, Value> = match existing {
        Some(text) => serde_json::from_str(text).map_err(|e| {
            CoreError::new(
                ErrorCode::Io,
                format!("settings.json is not valid JSON: {e}"),
            )
        })?,
        None => Map::new(),
    };
    let studio = root
        .entry("skill-studio")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(studio) = studio else {
        return Err(CoreError::new(
            ErrorCode::Io,
            "settings.json has a non-object `skill-studio` key",
        ));
    };
    let mut disabled_skills: Vec<String> = studio
        .get("disabledSkills")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect();
    disabled_skills.retain(|n| n != name);
    if disabled {
        disabled_skills.push(name.to_string());
    }
    disabled_skills.sort();
    if disabled_skills.is_empty() {
        studio.remove("disabledSkills");
        if studio.is_empty() {
            root.remove("skill-studio");
        }
    } else {
        studio.insert(
            "disabledSkills".to_string(),
            Value::Array(disabled_skills.into_iter().map(Value::String).collect()),
        );
    }

    serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| {
        CoreError::new(
            ErrorCode::Io,
            format!("failed to serialize settings.json: {e}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_add_then_remove_round_trips() {
        let denied = opencode_toggle(None, "find-bugs", true).unwrap();
        assert!(denied.contains("\"deny\""));
        let value: Value = serde_json::from_str(&denied).unwrap();
        assert_eq!(value["permission"]["skill"]["find-bugs"], "deny");
        let cleared = opencode_toggle(Some(&denied), "find-bugs", false).unwrap();
        let value: Value = serde_json::from_str(&cleared).unwrap();
        assert!(value.get("permission").is_none());
    }

    #[test]
    fn opencode_refuses_jsonc_only() {
        assert!(opencode_refuses_jsonc(false, true).is_err());
        assert!(opencode_refuses_jsonc(true, true).is_ok());
        assert!(opencode_refuses_jsonc(false, false).is_ok());
    }

    #[test]
    fn pi_add_then_remove_round_trips_and_cleans_up_the_empty_key() {
        let disabled = pi_toggle(None, "find-bugs", true).unwrap();
        let value: Value = serde_json::from_str(&disabled).unwrap();
        assert_eq!(value["skill-studio"]["disabledSkills"][0], "find-bugs");
        let enabled = pi_toggle(Some(&disabled), "find-bugs", false).unwrap();
        let value: Value = serde_json::from_str(&enabled).unwrap();
        assert!(value.get("skill-studio").is_none());
    }

    #[test]
    fn claude_override_off_then_removed_keeps_every_other_key_in_order_or_names_the_lost_entry() {
        let settings = r#"{
  "permissions": {"allow": ["Bash(ls)"]},
  "skillOverrides": {"code-review": "user-invocable-only"},
  "enabledPlugins": {"x@y": true}
}"#;
        let (off, earlier) = claude_skill_override_set(
            Some(settings),
            "find-bugs",
            Some(Value::String(CLAUDE_SKILL_OVERRIDE_OFF.into())),
        )
        .unwrap();
        assert_eq!(earlier, None, "find-bugs had no override before");
        let value: Value = serde_json::from_str(&off).unwrap();
        assert_eq!(value["skillOverrides"]["find-bugs"], "off");
        assert_eq!(
            value["skillOverrides"]["code-review"], "user-invocable-only",
            "the off write dropped another skill's override:\n{off}"
        );
        let keys: Vec<&str> = value
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["permissions", "skillOverrides", "enabledPlugins"],
            "the off write lost or reordered a top-level key"
        );

        let (on, earlier) = claude_skill_override_set(Some(&off), "find-bugs", None).unwrap();
        assert_eq!(earlier, Some(Value::String("off".into())));
        let value: Value = serde_json::from_str(&on).unwrap();
        assert!(
            value["skillOverrides"].get("find-bugs").is_none(),
            "the enable left find-bugs in skillOverrides:\n{on}"
        );
        assert_eq!(
            value["skillOverrides"]["code-review"],
            "user-invocable-only"
        );
    }

    #[test]
    fn claude_override_refuses_malformed_settings_instead_of_replacing_them() {
        assert!(
            claude_skill_override_set(Some("{not json"), "a", None).is_err(),
            "invalid JSON must be refused, not overwritten"
        );
        assert!(
            claude_skill_override_set(Some(r#"{"skillOverrides": []}"#), "a", None).is_err(),
            "a non-object skillOverrides must be refused, not overwritten"
        );
    }

    #[test]
    fn claude_override_removal_drops_an_empty_skill_overrides_object() {
        let (text, _) =
            claude_skill_override_set(Some(r#"{"skillOverrides": {"a": "off"}}"#), "a", None)
                .unwrap();
        let value: Value = serde_json::from_str(&text).unwrap();
        assert!(
            value.get("skillOverrides").is_none(),
            "an empty skillOverrides was left behind: {text}"
        );
    }
}
