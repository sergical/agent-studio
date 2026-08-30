// ============================================================================
// Skills Module - opencode_skill_permission
// Reads and writes OpenCode's own per-skill disable switch:
// `~/.config/opencode/opencode.json` `permission.skill.<name-or-glob> =
// "deny"`. OpenCode also accepts `opencode.jsonc` (with comments); Skill
// Studio never parses that format, so a project with only a `.jsonc` config
// is reported as unreadable (`OpencodeConfigKind::Jsonc`) rather than risking
// a write that drops the user's comments.
// ============================================================================

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// `~/.config/opencode/opencode.json`.
pub fn opencode_json_path(home: &Path) -> PathBuf {
    home.join(".config").join("opencode").join("opencode.json")
}

/// `~/.config/opencode/opencode.jsonc` - the sibling Skill Studio refuses to
/// parse or write.
pub fn opencode_jsonc_path(home: &Path) -> PathBuf {
    home.join(".config").join("opencode").join("opencode.jsonc")
}

/// Which OpenCode config format is present, so the frontend can tell the user
/// to hand-edit a `.jsonc` file rather than silently showing no disables.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OpencodeConfigKind {
    Json,
    Jsonc,
}

/// Which config file exists, if any - `None` when neither does (OpenCode
/// isn't configured, or uses its defaults).
pub fn detect_config_kind(home: &Path) -> Option<OpencodeConfigKind> {
    if opencode_json_path(home).is_file() {
        Some(OpencodeConfigKind::Json)
    } else if opencode_jsonc_path(home).is_file() {
        Some(OpencodeConfigKind::Jsonc)
    } else {
        None
    }
}

/// Every `permission.skill` pattern mapped to `"deny"`, or an empty set when
/// the file is missing, isn't JSON, or only a `.jsonc` sibling exists.
pub fn read_denied_patterns(home: &Path) -> Vec<String> {
    let Ok(content) = fs::read_to_string(opencode_json_path(home)) else {
        return Vec::new();
    };
    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(&content) else {
        return Vec::new();
    };
    let Some(Value::Object(skill)) = root
        .get("permission")
        .and_then(|p| p.as_object())
        .and_then(|p| p.get("skill"))
        .cloned()
    else {
        return Vec::new();
    };
    skill
        .into_iter()
        .filter(|(_, v)| v.as_str() == Some("deny"))
        .map(|(k, _)| k)
        .collect()
}

/// A `permission.skill` pattern matches `name` either exactly, or as a glob
/// with `*` as the only wildcard (e.g. `internal-*` matches `internal-foo`).
pub fn pattern_matches(pattern: &str, name: &str) -> bool {
    if !pattern.contains('*') {
        return pattern == name;
    }
    let mut rest = name;
    let mut parts = pattern.split('*').peekable();
    let mut first = true;
    while let Some(part) = parts.next() {
        if part.is_empty() {
            first = false;
            continue;
        }
        if first {
            let Some(after) = rest.strip_prefix(part) else {
                return false;
            };
            rest = after;
        } else if parts.peek().is_none() {
            // Last segment: must match the end of what's left.
            return rest.ends_with(part);
        } else {
            let Some(idx) = rest.find(part) else {
                return false;
            };
            rest = &rest[idx + part.len()..];
        }
        first = false;
    }
    true
}

/// Set (`deny`) or clear `permission.skill.<name>` in `opencode.json`,
/// preserving every other key and writing back pretty-printed with 2-space
/// indentation. Creates the file with the documented `$schema` when missing.
/// Refuses outright when only `opencode.jsonc` exists, since Skill Studio
/// must not silently create a `.json` sibling OpenCode would then have to
/// merge, nor rewrite the `.jsonc` and drop its comments.
pub fn set_skill_denied(home: &Path, name: &str, denied: bool) -> Result<(), String> {
    if opencode_jsonc_path(home).is_file() && !opencode_json_path(home).is_file() {
        return Err(
            "OpenCode's config is opencode.jsonc; edit permission.skill by hand".to_string(),
        );
    }

    let path = opencode_json_path(home);
    let mut root: Map<String, Value> = match fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content)
            .map_err(|e| format!("{} is not valid JSON: {e}", path.display()))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
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
        return Err(format!(
            "{} has a non-object `permission` key",
            path.display()
        ));
    };
    let skill = permission
        .entry("skill")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(skill) = skill else {
        return Err(format!(
            "{} has a non-object `permission.skill` key",
            path.display()
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

    let parent = path
        .parent()
        .ok_or("opencode.json has no parent directory")?;
    fs::create_dir_all(parent)
        .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    let json = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|e| format!("Failed to serialize opencode.json: {e}"))?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, json)
        .map_err(|e| format!("Failed to write {}: {e}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path).map_err(|e| {
        let _ = fs::remove_file(&tmp_path);
        format!("Failed to save {}: {e}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pattern_matches_exact_name() {
        assert!(pattern_matches("find-bugs", "find-bugs"));
        assert!(!pattern_matches("find-bugs", "write-tests"));
    }

    #[test]
    fn pattern_matches_star_glob() {
        assert!(pattern_matches("internal-*", "internal-foo"));
        assert!(!pattern_matches("internal-*", "external-foo"));
        assert!(pattern_matches("*-internal", "foo-internal"));
        assert!(pattern_matches("*", "anything"));
    }

    #[test]
    fn missing_file_has_no_denied_patterns() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(read_denied_patterns(tmp.path()).is_empty());
    }

    #[test]
    fn set_then_read_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        set_skill_denied(home, "find-bugs", true).unwrap();
        assert_eq!(read_denied_patterns(home), vec!["find-bugs".to_string()]);
    }

    #[test]
    fn creates_file_with_schema_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        set_skill_denied(home, "find-bugs", true).unwrap();
        let content = fs::read_to_string(opencode_json_path(home)).unwrap();
        assert!(content.contains("https://opencode.ai/config.json"));
    }

    #[test]
    fn clearing_removes_the_key_and_preserves_other_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(opencode_json_path(home).parent().unwrap()).unwrap();
        fs::write(
            opencode_json_path(home),
            r#"{"theme": "dark", "permission": {"skill": {"find-bugs": "deny"}}}"#,
        )
        .unwrap();

        set_skill_denied(home, "find-bugs", false).unwrap();
        let content = fs::read_to_string(opencode_json_path(home)).unwrap();
        let value: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(value["theme"], "dark");
        assert!(value.get("permission").is_none());
    }

    #[test]
    fn refuses_when_only_jsonc_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        fs::create_dir_all(opencode_jsonc_path(home).parent().unwrap()).unwrap();
        fs::write(opencode_jsonc_path(home), "// comment\n{}").unwrap();

        let err = set_skill_denied(home, "find-bugs", true).unwrap_err();
        assert!(err.contains("opencode.jsonc"));
        assert_eq!(detect_config_kind(home), Some(OpencodeConfigKind::Jsonc));
    }
}
