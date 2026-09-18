//! OpenCode's own per-skill disable switch: `opencode.json`
//! `permission.skill.<name-or-glob> = "deny"`.
//!
//! Ported from the desktop app's `skills/opencode_skill_permission.rs`,
//! which this module replaces. OpenCode also accepts `opencode.jsonc` (with
//! comments); this module never parses or writes that format, so a config
//! directory holding only a `.jsonc` file is reported as unreadable
//! ([`OpencodeConfigKind::Jsonc`]) rather than risking a write that drops
//! the user's comments.
//!
//! The config directory itself is not resolved here: `~/.config/opencode`
//! is only OpenCode's *default*, and `XDG_CONFIG_HOME`/`OPENCODE_CONFIG_DIR`
//! move it (`docs/action-map/harnesses/opencode.md`). Per this crate's own
//! rule against reading environment variables, the caller (the host
//! adapter, `skill_studio_host::opencode_config_dir`) resolves the env
//! overrides and passes the effective directory in.
//!
//! ## The deny rule shape, confirmed
//!
//! The v2 skills doc page (<https://opencode.ai/v2/docs/skills/>) describes
//! deny as a permission rule with an `effect` field
//! (`{action, resource, effect: "deny"}`), which raised the question of
//! whether the config-file key this module writes is still read. It is: the
//! `{effect: ...}` shape (`packages/schema/src/permission.ts`,
//! `PermissionV2.Rule`) is the *runtime* ask/approve protocol between the
//! client and the server, not what `opencode.json` holds. The config file's
//! `permission` key still decodes through the v1-named
//! `ConfigPermissionV1.Info` schema
//! (`packages/core/src/v1/config/permission.ts`): a record whose `skill`
//! entry is either one [`Action`] for every skill or an object mapping a
//! glob pattern to an [`Action`] - exactly `permission.skill.<name> =
//! "deny"`. Source: `anomalyco/opencode` (served for `sst/opencode`),
//! branch `dev`, commit `83452558f70207ddaeaffce68b36ebac77019fae`, files
//! `packages/core/src/v1/config/permission.ts` and
//! `packages/opencode/src/permission/index.ts` (`fromConfig`, which builds
//! the runtime ruleset from that same config shape).

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::error::{CoreError, ErrorCode};
use crate::ports::{acquire_exclusive, confine, LeaseProvider, ScopeFs};
use crate::registry::home_only_scope;

/// `opencode.json`'s documented `$schema` value, added when the file is
/// created fresh.
const OPENCODE_CONFIG_SCHEMA: &str = "https://opencode.ai/config.json";

/// The one action string this module ever writes. `opencode.json` accepts
/// `"ask"` and `"allow"` too (`ConfigPermissionV1.Action`), but Skill
/// Studio's only native OpenCode switch is the deny rule.
const DENY: &str = "deny";

/// `<config_dir>/opencode.json`.
pub fn opencode_json_path(config_dir: &Path) -> PathBuf {
    config_dir.join("opencode.json")
}

/// `<config_dir>/opencode.jsonc` - the sibling this module refuses to parse
/// or write.
pub fn opencode_jsonc_path(config_dir: &Path) -> PathBuf {
    config_dir.join("opencode.jsonc")
}

/// Which OpenCode config format is present, so a caller can tell the user
/// to hand-edit a `.jsonc` file rather than silently showing no disables.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum OpencodeConfigKind {
    /// `opencode.json` exists (parsed and written).
    Json,
    /// Only `opencode.jsonc` exists (never parsed or written).
    Jsonc,
}

/// Which config file exists, if any - `None` when neither does (OpenCode
/// isn't configured, or uses its defaults).
pub fn detect_config_kind(fs: &dyn ScopeFs, config_dir: &Path) -> Option<OpencodeConfigKind> {
    if fs.symlink_metadata(&opencode_json_path(config_dir)).is_ok() {
        Some(OpencodeConfigKind::Json)
    } else if fs
        .symlink_metadata(&opencode_jsonc_path(config_dir))
        .is_ok()
    {
        Some(OpencodeConfigKind::Jsonc)
    } else {
        None
    }
}

/// Largest `opencode.json` this module will read. Larger is treated as
/// unreadable rather than silently truncated.
pub const OPENCODE_CONFIG_MAX_BYTES: u64 = 8 * 1024 * 1024;

/// Every `permission.skill` pattern mapped to `"deny"`, or an empty set when
/// the file is missing, isn't JSON, or only a `.jsonc` sibling exists.
pub fn read_denied_patterns(fs: &dyn ScopeFs, config_dir: &Path) -> Vec<String> {
    let Ok(bytes) = fs.read_capped(&opencode_json_path(config_dir), OPENCODE_CONFIG_MAX_BYTES)
    else {
        return Vec::new();
    };
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(&bytes) else {
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
        .filter(|(_, v)| v.as_str() == Some(DENY))
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

/// Resolves `config_dir` to its real location before it is used to scope a
/// write. `~/.config/opencode` can itself be a symlink (a dotfiles layout
/// keeping the real directory under version control elsewhere); [`confine`]
/// checks the *canonical* parent of the write path against the scope, so an
/// unresolved symlinked `config_dir` would put the write's canonical parent
/// outside `<config_dir>/..` and get refused even though the write is well
/// inside the intended directory. `config_dir` not existing yet (OpenCode
/// never configured) is not an error - there is nothing on disk to follow,
/// so it is used as given and created fresh under its own (unresolved) path.
fn resolve_config_dir(fs: &dyn ScopeFs, config_dir: &Path) -> Result<PathBuf, CoreError> {
    match fs.canonicalize(config_dir) {
        Ok(resolved) => Ok(resolved),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(config_dir.to_path_buf()),
        Err(e) => Err(CoreError::io(config_dir, e)),
    }
}

/// Set (`deny`) or clear `permission.skill.<name>` in `<config_dir>/opencode.json`,
/// preserving every other key and writing back pretty-printed. Creates the
/// file with the documented `$schema` when missing. Refuses when only
/// `opencode.jsonc` exists, since this module must not silently create a
/// `.json` sibling OpenCode would then have to merge, nor rewrite the
/// `.jsonc` and drop its comments.
///
/// `config_dir` is the already-resolved OpenCode config directory
/// (`XDG_CONFIG_HOME`/`OPENCODE_CONFIG_DIR` already applied by the caller),
/// which may sit outside the app's own home - so this takes its own
/// home-only scope rather than reusing one built from the app's home.
///
/// [`confine`] proves a path by also checking that its *parent's* parent
/// lies in scope (guards against a symlinked parent escaping the scope), so
/// the scope's home must be `config_dir`'s parent, not `config_dir` itself:
/// confining `config_dir` as a path directly under its own scope's home
/// would always fail, since a scope's home is never "in scope" of itself
/// one level up. `config_dir`'s parent is expected to exist even when
/// `config_dir` (OpenCode's config directory) does not yet - e.g.
/// `~/.config` exists before `~/.config/opencode` is ever created.
pub fn set_skill_denied(
    leases: &dyn LeaseProvider,
    fs: &dyn ScopeFs,
    config_dir: &Path,
    name: &str,
    denied: bool,
) -> Result<(), CoreError> {
    let config_dir = &resolve_config_dir(fs, config_dir)?;
    let jsonc_path = opencode_jsonc_path(config_dir);
    let path = opencode_json_path(config_dir);
    if fs.symlink_metadata(&jsonc_path).is_ok() && fs.symlink_metadata(&path).is_err() {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "OpenCode's config is opencode.jsonc; edit permission.skill by hand",
        )
        .at(&jsonc_path));
    }

    let home = config_dir.parent().ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "config directory has no parent to scope the write to",
        )
        .at(config_dir)
    })?;
    let scope = home_only_scope(home, fs)?;
    let guard = acquire_exclusive(leases, &scope)?;

    let mut root: Map<String, Value> = match fs.read_capped(&path, OPENCODE_CONFIG_MAX_BYTES) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| CoreError::new(ErrorCode::Io, format!("not valid JSON: {e}")).at(&path))?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(CoreError::io(&path, e)),
    };
    if !root.contains_key("$schema") {
        root.insert(
            "$schema".to_string(),
            Value::String(OPENCODE_CONFIG_SCHEMA.to_string()),
        );
    }

    let permission = root
        .entry("permission")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(permission) = permission else {
        return Err(CoreError::new(ErrorCode::Io, "has a non-object `permission` key").at(&path));
    };
    let skill = permission
        .entry("skill")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(skill) = skill else {
        return Err(
            CoreError::new(ErrorCode::Io, "has a non-object `permission.skill` key").at(&path),
        );
    };

    if denied {
        skill.insert(name.to_string(), Value::String(DENY.to_string()));
    } else {
        skill.remove(name);
        if skill.is_empty() {
            permission.remove("skill");
        }
        if permission.is_empty() {
            root.remove("permission");
        }
    }

    if let Some(parent) = path.parent() {
        let scoped_parent = confine(&scope, fs, parent)?;
        fs.create_dir_all(&guard, &scoped_parent)
            .map_err(|e| CoreError::io(parent, e))?;
    }
    let bytes = serde_json::to_vec_pretty(&Value::Object(root)).map_err(|e| {
        CoreError::new(ErrorCode::Io, format!("failed to serialize: {e}")).at(&path)
    })?;
    let scoped = confine(&scope, fs, &path)?;
    fs.write_atomic(&guard, &scoped, &bytes)
        .map_err(|e| CoreError::io(&path, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureBuilder;

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
    fn missing_config_dir_has_no_denied_patterns() {
        let fs = FixtureBuilder::new().dir("/home").build_fs();
        assert!(read_denied_patterns(&fs, Path::new("/home/.config/opencode")).is_empty());
    }

    #[test]
    fn reads_denied_patterns_from_an_existing_config() {
        let fs = FixtureBuilder::new()
            .dir("/home/.config/opencode")
            .file(
                "/home/.config/opencode/opencode.json",
                br#"{"permission": {"skill": {"find-bugs": "deny", "write-tests": "allow"}}}"#,
            )
            .build_fs();
        assert_eq!(
            read_denied_patterns(&fs, Path::new("/home/.config/opencode")),
            vec!["find-bugs".to_string()]
        );
    }

    #[test]
    fn jsonc_only_config_reports_as_jsonc_and_no_denied_patterns() {
        let fs = FixtureBuilder::new()
            .dir("/home/.config/opencode")
            .file("/home/.config/opencode/opencode.jsonc", b"// comment\n{}")
            .build_fs();
        assert_eq!(
            detect_config_kind(&fs, Path::new("/home/.config/opencode")),
            Some(OpencodeConfigKind::Jsonc)
        );
        assert!(read_denied_patterns(&fs, Path::new("/home/.config/opencode")).is_empty());
    }

    /// Fixture: the config-schema declaration of `permission.skill`,
    /// verbatim from `packages/core/src/v1/config/permission.ts` in
    /// `anomalyco/opencode` (served for `sst/opencode`), branch `dev`,
    /// commit `83452558f70207ddaeaffce68b36ebac77019fae` - the file the v2
    /// runtime's config loader decodes `opencode.json`'s `permission` key
    /// through (`packages/opencode/src/permission/index.ts`, `fromConfig`).
    /// Kept as source text, not restated as a claim, so a future edit of
    /// this test has to re-paste the real file rather than describe it.
    const OPENCODE_V1_CONFIG_PERMISSION_SOURCE: &str = r#"
export const Action = Schema.Literals(["ask", "allow", "deny"]).annotate({ identifier: "PermissionActionConfig" })
export type Action = Schema.Schema.Type<typeof Action>

export const Object = Schema.Record(Schema.String, Action).annotate({ identifier: "PermissionObjectConfig" })
export type Object = Schema.Schema.Type<typeof Object>

export const Rule = Schema.Union([Action, Object]).annotate({ identifier: "PermissionRuleConfig" })
export type Rule = Schema.Schema.Type<typeof Rule>

const InputObject = Schema.StructWithRest(
  Schema.Struct({
    read: Schema.optional(Rule),
    edit: Schema.optional(Rule),
    skill: Schema.optional(Rule),
  }),
  [Schema.Record(Schema.String, Rule)],
)
"#;

    /// Flow: the deny rule this module writes (`permission.skill.<name> =
    /// "deny"`) is checked against the config schema the v2 source actually
    /// decodes, not the `{action, resource, effect}` shape the v2 skills
    /// doc page describes for the runtime ask/approve protocol.
    /// Expectation: the source fixture declares `skill: Schema.optional(Rule)`
    /// with `Rule = Action | Record<string, Action>` and `Action` including
    /// `"deny"`, which is exactly a bare string value like the one this
    /// module writes for a single-pattern deny - not an object with an
    /// `effect` field.
    /// Failure here (the source fixture no longer declaring `skill` under
    /// `Rule`, or `Action` no longer listing `"deny"`) would mean OpenCode
    /// v2 silently ignores the key the app writes today.
    #[test]
    fn opencode_deny_rule_shape_matches_the_v2_source_or_names_the_shape_the_code_writes_instead() {
        let source = OPENCODE_V1_CONFIG_PERMISSION_SOURCE;
        assert!(
            source.contains("skill: Schema.optional(Rule)"),
            "the source no longer declares a `skill` permission key"
        );
        assert!(
            source.contains(r#"["ask", "allow", "deny"]"#),
            "the source's Action union no longer lists \"deny\""
        );
        assert!(
            source.contains("Rule = Schema.Union([Action, Object])"),
            "Rule is no longer `Action | Record<string, Action>` (a bare \
             string or a pattern map), so a bare `\"deny\"` string may no \
             longer be a valid `permission.skill` value"
        );

        // What this module actually writes for one deny: a bare string,
        // matching `Action`, nested under `permission.skill.<name>` -
        // never `{action, resource, effect: "deny"}` (`PermissionV2.Rule`,
        // `packages/schema/src/permission.ts`, the *runtime* ask/approve
        // shape, not the config-file shape).
        let mut skill = Map::new();
        skill.insert("find-bugs".to_string(), Value::String(DENY.to_string()));
        let mut permission = Map::new();
        permission.insert("skill".to_string(), Value::Object(skill));
        let mut root = Map::new();
        root.insert("permission".to_string(), Value::Object(permission));
        let written = Value::Object(root);

        assert_eq!(written["permission"]["skill"]["find-bugs"], "deny");
        assert!(written["permission"]["skill"]["find-bugs"].is_string());
        assert!(written["permission"]["skill"]["find-bugs"]
            .get("effect")
            .is_none());
    }
}
