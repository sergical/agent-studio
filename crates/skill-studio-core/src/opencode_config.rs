//! `OpenCode`'s own per-skill disable switch: `opencode.json`
//! `permission.skill.<name-or-glob> = "deny"`.
//!
//! Ported from the desktop app's `skills/opencode_skill_permission.rs`,
//! which this module replaces. `OpenCode` also accepts `opencode.jsonc` (with
//! comments); this module never parses or writes that format, so a config
//! directory holding only a `.jsonc` file is reported as unreadable
//! ([`OpencodeConfigKind::Jsonc`]) rather than risking a write that drops
//! the user's comments.
//!
//! The config directory itself is not resolved here: `~/.config/opencode`
//! is only `OpenCode`'s *default*, and `XDG_CONFIG_HOME`/`OPENCODE_CONFIG_DIR`
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
use crate::ports::{acquire_exclusive, confine, ExclusiveGuard, LeaseProvider, ScopeFs};
use crate::registry::home_only_scope;

/// `opencode.json`'s documented `$schema` value, added when the file is
/// created fresh.
const OPENCODE_CONFIG_SCHEMA: &str = "https://opencode.ai/config.json";

/// The one action string this module ever writes. `opencode.json` accepts
/// `"ask"` and `"allow"` too (`ConfigPermissionV1.Action`), but Skill
/// Studio's only native `OpenCode` switch is the deny rule.
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

/// Which `OpenCode` config format is present, so a caller can tell the user
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

/// Which config file exists, if any - `None` when neither does (`OpenCode`
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

/// One `permission.skill` (v1) or `permissions[]` (v2) rule: a pattern (or
/// `resource` glob) paired with the effect it applies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRule {
    /// The pattern (v1 key, or v2 `resource`) matched against a skill name.
    pub pattern: String,
    /// The rule's `Action`/`effect`: `"ask"`, `"allow"`, or `"deny"`.
    pub effect: String,
}

/// Every skill-permission rule `opencode.json` holds, split by the config
/// generation that produced it. `OpenCode` reads both generations on `dev`
/// (the v1→v2 migration doc says v1 syntax is still accepted), so a config
/// file can hold either shape, or - in principle - both at once.
///
/// Cross-shape precedence (what happens when both `permission.skill` and a
/// `permissions[]` skill rule name the same skill with different effects) is
/// not verified against the `OpenCode` source; [`is_denied`] treats the two
/// lists as independent gates - deny in either one denies the skill - as an
/// assumption pending that follow-up.
///
/// [`is_denied`]: OpencodeSkillRules::is_denied
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OpencodeSkillRules {
    /// From `permission.skill`: a bare string becomes one rule for `*`;
    /// an object's entries become one rule per key, in document order.
    pub v1: Vec<SkillRule>,
    /// From top-level `permissions[]`, every entry whose `action == "skill"`,
    /// in document order (`resource` becomes the pattern).
    pub v2: Vec<SkillRule>,
}

/// Last-match-wins evaluation of `rules` against `name`, defaulting to
/// `false` (not denied) when nothing matches - `OpenCode`'s own default is
/// `ask`, and this module only distinguishes "denied" from "not denied".
fn rules_deny(rules: &[SkillRule], name: &str) -> bool {
    rules
        .iter()
        .rfind(|rule| pattern_matches(&rule.pattern, name))
        .is_some_and(|rule| rule.effect == DENY)
}

impl OpencodeSkillRules {
    /// `name` is denied when either shape's last matching rule is `deny`.
    pub fn is_denied(&self, name: &str) -> bool {
        rules_deny(&self.v1, name) || rules_deny(&self.v2, name)
    }
}

/// One `permission.skill` value (`ConfigPermissionV1.Rule`: a bare `Action`
/// string, applying to every skill, or an object mapping a pattern to an
/// `Action`) turned into ordered [`SkillRule`]s.
fn v1_skill_rules(skill: &Value) -> Vec<SkillRule> {
    match skill {
        Value::String(effect) => vec![SkillRule {
            pattern: "*".to_string(),
            effect: effect.clone(),
        }],
        Value::Object(map) => map
            .iter()
            .filter_map(|(pattern, effect)| {
                effect.as_str().map(|effect| SkillRule {
                    pattern: pattern.clone(),
                    effect: effect.to_string(),
                })
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Every v2 `permissions[]` entry whose `action == "skill"`, in document
/// order.
fn v2_skill_rules(root: &Map<String, Value>) -> Vec<SkillRule> {
    let Some(Value::Array(entries)) = root.get("permissions") else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let entry = entry.as_object()?;
            if entry.get("action").and_then(Value::as_str) != Some("skill") {
                return None;
            }
            let pattern = entry.get("resource").and_then(Value::as_str)?.to_string();
            let effect = entry.get("effect").and_then(Value::as_str)?.to_string();
            Some(SkillRule { pattern, effect })
        })
        .collect()
}

/// Reads `<config_dir>/opencode.json`'s `permission.skill` (v1) and
/// `permissions[]` skill rules (v2), or two empty lists when the file is
/// missing, isn't JSON, or only a `.jsonc` sibling exists.
pub fn read_skill_rules(fs: &dyn ScopeFs, config_dir: &Path) -> OpencodeSkillRules {
    let Ok(bytes) = fs.read_capped(&opencode_json_path(config_dir), OPENCODE_CONFIG_MAX_BYTES)
    else {
        return OpencodeSkillRules::default();
    };
    let Ok(Value::Object(root)) = serde_json::from_slice::<Value>(&bytes) else {
        return OpencodeSkillRules::default();
    };
    let v1 = root
        .get("permission")
        .and_then(Value::as_object)
        .and_then(|p| p.get("skill"))
        .map(v1_skill_rules)
        .unwrap_or_default();
    let v2 = v2_skill_rules(&root);
    OpencodeSkillRules { v1, v2 }
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
/// inside the intended directory. `config_dir` not existing yet (`OpenCode`
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
/// `.json` sibling `OpenCode` would then have to merge, nor rewrite the
/// `.jsonc` and drop its comments.
///
/// `config_dir` is the already-resolved `OpenCode` config directory
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
/// `config_dir` (`OpenCode`'s config directory) does not yet - e.g.
/// `~/.config` exists before `~/.config/opencode` is ever created.
pub fn set_skill_denied(
    leases: &dyn LeaseProvider,
    fs: &dyn ScopeFs,
    config_dir: &Path,
    name: &str,
    denied: bool,
) -> Result<(), CoreError> {
    let config_dir = resolve_config_dir(fs, config_dir)?;
    let home = config_dir.parent().ok_or_else(|| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            "config directory has no parent to scope the write to",
        )
        .at(&config_dir)
    })?;
    let scope = home_only_scope(home, fs)?;
    let guard = acquire_exclusive(leases, &scope)?;
    set_skill_denied_with(fs, &guard, &config_dir, name, denied)
}

/// The lease-holding half of [`set_skill_denied`]. This trusts the caller to
/// already hold an exclusive lease keyed to `config_dir`'s canonical parent -
/// it does not verify coverage, and acquiring a second lease on that same
/// root here would self-deadlock a caller whose held lease already covers
/// it (advisory locks do not nest in-process).
///
/// A caller whose held lease is scoped to a different root - the desktop
/// command's `WriteLease`, keyed to the app's `home`, does not cover
/// `config_dir`'s parent unless `OPENCODE_CONFIG_DIR`/`XDG_CONFIG_HOME`
/// happens to point `config_dir` directly under `home` - must check
/// `guard.keys()` for that coverage itself before calling this rather than
/// [`set_skill_denied`], and fall back to [`set_skill_denied`] (which
/// acquires its own correctly-scoped lease) when it isn't covered; see
/// `skill_harness_disable.rs`'s `OpenCode` arm.
///
/// `config_dir` need not already be resolved - this re-resolves it itself
/// (see [`resolve_config_dir`]; a no-op when it's already canonical), the
/// same as [`set_skill_denied`] does before acquiring its own guard.
pub fn set_skill_denied_with(
    fs: &dyn ScopeFs,
    guard: &ExclusiveGuard,
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

    // Computed before the mutable borrows below: clearing the v1 key
    // doesn't help when a v2 `permissions[]` rule still denies `name` -
    // reporting success here would show the skill enabled while `OpenCode`
    // keeps refusing it. Name the matching v2 rule's `resource` so the UI
    // can point at what still needs editing, and leave the array untouched
    // (this module never writes v2).
    let blocking_v2_rule = (!denied)
        .then(|| v2_skill_rules(&root))
        .into_iter()
        .flatten()
        .rfind(|rule| pattern_matches(&rule.pattern, name))
        .filter(|rule| rule.effect == DENY);
    if let Some(rule) = blocking_v2_rule {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            format!(
                "still denied by permissions[] rule for \"{}\"; edit opencode.json by hand",
                rule.pattern
            ),
        )
        .at(&path));
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
        // `Map::insert` on an existing key updates its value in place,
        // keeping its old position - so a skill re-disabled after being
        // moved earlier in the file (e.g. by a hand-edit) would still lose
        // to a later `allow`/`deny` rule for an overlapping pattern.
        // `shift_remove` then `insert` puts it back at the end, so this
        // module's own deny rule is always the document's last (and so
        // last-match-wins) rule for `name`.
        skill.shift_remove(name);
        skill.insert(name.to_string(), Value::String(DENY.to_string()));
    } else {
        // `Map::remove` is `swap_remove` under `preserve_order` (moves the
        // map's last entry into the removed slot), which would reorder
        // every rule after `name` in the user's file. `shift_remove` keeps
        // every other rule's document order.
        skill.shift_remove(name);

        // Removing `name`'s own key doesn't help when an earlier glob rule
        // (e.g. `"*": "deny"`) still matches it - reporting success here
        // would show the skill enabled while `OpenCode` keeps denying it
        // through that rule. Re-evaluate the v1 rules with `name`'s own key
        // already gone, and refuse (without writing) if one still denies.
        let remaining_v1_rules = v1_skill_rules(&Value::Object(skill.clone()));
        if let Some(rule) = remaining_v1_rules
            .iter()
            .rfind(|rule| pattern_matches(&rule.pattern, name))
            .filter(|rule| rule.effect == DENY)
        {
            return Err(CoreError::new(
                ErrorCode::Unsupported,
                format!(
                    "still denied by permission.skill rule for \"{}\"; edit opencode.json by hand",
                    rule.pattern
                ),
            )
            .at(&path));
        }

        if skill.is_empty() {
            permission.remove("skill");
        }
        if permission.is_empty() {
            root.remove("permission");
        }
    }

    if let Some(parent) = path.parent() {
        let scoped_parent = confine(&scope, fs, parent)?;
        fs.create_dir_all(guard, &scoped_parent)
            .map_err(|e| CoreError::io(parent, e))?;
    }
    let bytes = serde_json::to_vec_pretty(&Value::Object(root)).map_err(|e| {
        CoreError::new(ErrorCode::Io, format!("failed to serialize: {e}")).at(&path)
    })?;
    let scoped = confine(&scope, fs, &path)?;
    fs.write_atomic(guard, &scoped, &bytes)
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
        let rules = read_skill_rules(&fs, Path::new("/home/.config/opencode"));
        assert!(!rules.is_denied("anything"));
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
        let rules = read_skill_rules(&fs, Path::new("/home/.config/opencode"));
        assert!(rules.is_denied("find-bugs"));
        assert!(!rules.is_denied("write-tests"));
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
        let rules = read_skill_rules(&fs, Path::new("/home/.config/opencode"));
        assert!(!rules.is_denied("anything"));
    }

    /// Flow: a v2 `permissions[]` deny rule for a skill.
    /// Expectation: `is_denied` denies the matching skill and leaves an
    /// unmatched one enabled.
    /// Failure: a skill either shown enabled despite the rule, or the rule
    /// wrongly matching a skill outside its pattern.
    #[test]
    fn a_v2_permissions_array_deny_rule_disables_the_skill_or_names_the_skill_shown_enabled() {
        let fs = FixtureBuilder::new()
            .dir("/home/.config/opencode")
            .file(
                "/home/.config/opencode/opencode.json",
                br#"{"permissions":[{"action":"skill","resource":"eps*","effect":"deny"}]}"#,
            )
            .build_fs();
        let rules = read_skill_rules(&fs, Path::new("/home/.config/opencode"));
        assert!(
            rules.is_denied("epsilon"),
            "epsilon (matching \"eps*\") reported enabled"
        );
        assert!(
            !rules.is_denied("alpha"),
            "alpha (not matching \"eps*\") reported denied"
        );
    }

    /// Flow: two v2 rules for the same skill in opposite orders.
    /// Expectation: the later rule wins either way.
    /// Failure: the earlier rule wins instead, i.e. rule order is ignored.
    #[test]
    fn a_later_v2_rule_overrides_an_earlier_one_or_names_the_rule_it_ignored() {
        let fs = FixtureBuilder::new()
            .dir("/home/.config/opencode")
            .file(
                "/home/.config/opencode/opencode.json",
                br#"{"permissions":[{"action":"skill","resource":"*","effect":"deny"},{"action":"skill","resource":"epsilon","effect":"allow"}]}"#,
            )
            .build_fs();
        let rules = read_skill_rules(&fs, Path::new("/home/.config/opencode"));
        assert!(
            !rules.is_denied("epsilon"),
            "later \"allow epsilon\" rule was ignored"
        );

        let fs = FixtureBuilder::new()
            .dir("/home/.config/opencode")
            .file(
                "/home/.config/opencode/opencode.json",
                br#"{"permissions":[{"action":"skill","resource":"epsilon","effect":"allow"},{"action":"skill","resource":"*","effect":"deny"}]}"#,
            )
            .build_fs();
        let rules = read_skill_rules(&fs, Path::new("/home/.config/opencode"));
        assert!(
            rules.is_denied("epsilon"),
            "later \"deny *\" rule was ignored"
        );
    }

    /// Flow: `set_skill_denied(false)` on a skill a v2 `permissions[]` rule
    /// still denies.
    /// Expectation: the call is refused and names the rule, rather than
    /// reporting success while `OpenCode` keeps refusing the skill.
    /// Failure: `Ok(())` returned while the skill stays denied.
    #[test]
    fn enabling_a_skill_denied_by_a_v2_rule_is_refused_and_names_the_rule_or_names_the_skill_it_reported_enabled(
    ) {
        let fs = FixtureBuilder::new()
            .dir("/home/.config/opencode")
            .file(
                "/home/.config/opencode/opencode.json",
                br#"{"permission":{"skill":{"epsilon":"deny"}},"permissions":[{"action":"skill","resource":"eps*","effect":"deny"}]}"#,
            )
            .build_fs();
        let leases = crate::testing::FakeLease::default();
        let err = set_skill_denied(
            &leases,
            &fs,
            Path::new("/home/.config/opencode"),
            "epsilon",
            false,
        )
        .expect_err("epsilon reported enabled despite the surviving \"eps*\" v2 rule");
        assert!(
            err.to_string().contains("eps*"),
            "error {err} doesn't name the blocking rule"
        );
    }

    /// Flow: `set_skill_denied(false)` on `zeta`, starting from
    /// `{"permission": {"skill": {"z*": "deny", "zeta": "deny"}}}` - `zeta`'s
    /// own key is removed, but the glob `"z*"` still matches it.
    /// Expectation: the enable is refused, naming `"z*"` as the rule still
    /// denying it, and nothing is written.
    /// Failure: the enable silently succeeds (or reports success) while
    /// `OpenCode` keeps denying `zeta` through `"z*"`.
    #[test]
    fn enabling_a_skill_still_denied_by_a_v1_glob_rule_is_refused_or_names_the_pattern_it_ignored()
    {
        let fs = FixtureBuilder::new()
            .dir("/home/.config/opencode")
            .file(
                "/home/.config/opencode/opencode.json",
                br#"{"permission":{"skill":{"z*":"deny","zeta":"deny"}}}"#,
            )
            .build_fs();
        let leases = crate::testing::FakeLease::default();
        let err = set_skill_denied(
            &leases,
            &fs,
            Path::new("/home/.config/opencode"),
            "zeta",
            false,
        )
        .expect_err("zeta reported enabled despite the surviving \"z*\" v1 rule");
        assert!(
            err.to_string().contains("z*"),
            "error {err} doesn't name the blocking rule"
        );
    }

    /// Flow: `permission.skill = "deny"` (a bare string, not an object).
    /// Expectation: every skill is denied, matching `ConfigPermissionV1.Rule
    /// = Action | Object` - a bare `Action` applies to every skill.
    /// Failure: the bare string silently yields no denied skills.
    #[test]
    fn a_bare_skill_deny_string_denies_every_skill_or_names_the_skill_it_let_through() {
        let fs = FixtureBuilder::new()
            .dir("/home/.config/opencode")
            .file(
                "/home/.config/opencode/opencode.json",
                br#"{"permission": {"skill": "deny"}}"#,
            )
            .build_fs();
        let rules = read_skill_rules(&fs, Path::new("/home/.config/opencode"));
        assert!(
            rules.is_denied("anything"),
            "a bare \"deny\" string let \"anything\" through"
        );
    }

    /// Flow: `{"zzz": "deny", "*": "allow"}` - key order (`zzz` then `*`) is
    /// deliberately the *opposite* of sorted key order (`*` sorts before
    /// `zzz`), so a reader that iterates a `serde_json::Map` in sorted
    /// order (the default without `preserve_order`) would evaluate `*` as
    /// if it came first and `zzz`'s `deny` as the last, document-order
    /// match.
    /// Expectation: `zzz` reads as allowed - the wildcard `allow` written
    /// *after* it in the document is the real last match - and `bar`
    /// (caught only by the wildcard) reads as allowed too.
    /// Failure: `zzz` reported denied because the reader used sorted, not
    /// document, key order.
    #[test]
    fn read_skill_rules_honours_document_key_order_not_sorted_order_or_names_the_skill_it_reported_denied_by_mistake(
    ) {
        let fs = FixtureBuilder::new()
            .dir("/home/.config/opencode")
            .file(
                "/home/.config/opencode/opencode.json",
                br#"{"permission": {"skill": {"zzz": "deny", "*": "allow"}}}"#,
            )
            .build_fs();
        let rules = read_skill_rules(&fs, Path::new("/home/.config/opencode"));
        assert!(
            !rules.is_denied("zzz"),
            "zzz reported denied despite the later \"*\": \"allow\" entry"
        );
        assert!(
            !rules.is_denied("bar"),
            "bar reported denied despite the \"*\": \"allow\" entry"
        );
    }

    /// Flow: `{"foo": "allow", "*": "deny"}` - a specific allow written
    /// before a wildcard deny.
    /// Expectation: `foo` reads as denied - the wildcard `deny` is the
    /// later, document-order rule, and last-match-wins ignores which rule
    /// is more specific.
    /// Failure: `foo` reported allowed (or the ignored rule named) because
    /// the reader picked the specific `allow` over the later `deny`.
    #[test]
    fn a_v1_deny_rule_written_after_an_allow_rule_wins_or_names_the_rule_order_it_ignored() {
        let fs = FixtureBuilder::new()
            .dir("/home/.config/opencode")
            .file(
                "/home/.config/opencode/opencode.json",
                br#"{"permission": {"skill": {"foo": "allow", "*": "deny"}}}"#,
            )
            .build_fs();
        let rules = read_skill_rules(&fs, Path::new("/home/.config/opencode"));
        assert!(
            rules.is_denied("foo"),
            "foo reported allowed despite the later \"*\": \"deny\" entry"
        );
    }
}
