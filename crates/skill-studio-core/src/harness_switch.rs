//! Shared pieces of the native per-skill switches that
//! [`crate::ops::set_harness_enabled`] writes, kept free of `std::fs`.
//!
//! The edits themselves live beside their readers so one writer owns each
//! file: Codex's `[[skills.config]]` row is `crate::ops::codex_write_disabled_row`
//! and `OpenCode`'s `permission.skill` deny is
//! `crate::opencode_config::skill_denied_text`. Claude Code's
//! `skillOverrides` edit is [`claude_skill_override_set`], here. pi, Cursor,
//! and Grok Build have no switch Skill Studio writes; Park is their off path.

use crate::error::{CoreError, ErrorCode};
use serde_json::{Map, Value};

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opencode_refuses_jsonc_only() {
        assert!(opencode_refuses_jsonc(false, true).is_err());
        assert!(opencode_refuses_jsonc(true, true).is_ok());
        assert!(opencode_refuses_jsonc(false, false).is_ok());
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
