// ============================================================================
// Skills Module - Plugin Enumeration
// Discovers skills shipped inside agent plugins (Claude Code, Codex, and any
// agent-plugins.org-shaped plugin) by walking plugin cache trees for
// manifests and their bundled skills/ directories.
//
// OpenCode and pi plugins are JS/TS extension modules, not Agent Skills
// bundles - they have no skills/ subdirectory to scan, so we don't scan them.
// ============================================================================

use std::fs;
use std::path::{Path, PathBuf};

use super::skill_dto::PluginInfo;

/// Plugin manifest filenames to look for, in priority order: the
/// harness-scoped folder (`.claude-plugin/plugin.json`,
/// `.codex-plugin/plugin.json`) first, then the generic agent-plugins.org
/// root `plugin.json`.
const MANIFEST_CANDIDATES: &[&str] = &[
    ".claude-plugin/plugin.json",
    ".codex-plugin/plugin.json",
    "plugin.json",
];

/// A plugin root discovered while walking a cache tree, plus the skills it ships.
pub struct EnumeratedPlugin {
    pub info: PluginInfo,
    pub root: PathBuf,
}

/// A single skill directory found inside a plugin's `skills/` subdirectory.
pub struct PluginSkillDir {
    pub skill_dir: PathBuf,
    pub plugin: PluginInfo,
}

/// Parse a plugin.json manifest (agent-plugins.org shape: `$schema`, `name`
/// required; `version`/`description` optional). Parsed leniently -
/// unrecognized fields are ignored and a missing/unreadable/malformed
/// manifest just yields `None` rather than failing the whole scan.
fn read_plugin_manifest(manifest_path: &Path) -> Option<(String, Option<String>)> {
    let content = fs::read_to_string(manifest_path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&content).ok()?;
    let name = value.get("name")?.as_str()?.to_string();
    let version = value
        .get("version")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some((name, version))
}

/// Check whether `dir` is a plugin root (one of the recognized manifest
/// files directly inside it). Falls back to the directory name when the
/// manifest exists but doesn't parse.
fn manifest_in(dir: &Path) -> Option<(String, Option<String>)> {
    for candidate in MANIFEST_CANDIDATES {
        let manifest_path = dir.join(candidate);
        if manifest_path.is_file() {
            return Some(read_plugin_manifest(&manifest_path).unwrap_or_else(|| {
                (
                    dir.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                    None,
                )
            }));
        }
    }
    None
}

/// Walk up from `path` looking for a plugin root. Used to identify
/// plugin-shipped skills found via a generic directory scan (e.g. a skill
/// dir reached through `.agents/skills` or an agent's own skills root),
/// not just the dedicated plugin cache walk below.
pub fn find_plugin_root(path: &Path, harness: &str) -> Option<PluginInfo> {
    let mut current = Some(path);
    while let Some(dir) = current {
        if let Some((name, version)) = manifest_in(dir) {
            return Some(PluginInfo {
                name,
                version,
                harness: harness.to_string(),
            });
        }
        current = dir.parent();
    }
    None
}

/// Walk a plugin cache tree up to `max_depth` levels looking for plugin
/// roots. Handles the documented Claude Code layout
/// (`cache/{marketplace}/{plugin}/{version}/`) as well as shallower ones,
/// since real caches vary. Stops descending once a plugin root is found.
fn find_plugin_roots(cache_dir: &Path, harness: &str, max_depth: u8) -> Vec<EnumeratedPlugin> {
    let mut found = Vec::new();
    walk_for_plugin_roots(cache_dir, harness, max_depth, &mut found);
    found
}

fn walk_for_plugin_roots(
    dir: &Path,
    harness: &str,
    depth_remaining: u8,
    found: &mut Vec<EnumeratedPlugin>,
) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        if let Some((name, version)) = manifest_in(&path) {
            found.push(EnumeratedPlugin {
                info: PluginInfo {
                    name,
                    version,
                    harness: harness.to_string(),
                },
                root: path,
            });
            continue; // a plugin root's own subdirectories aren't further plugin roots
        }
        if depth_remaining > 0 {
            walk_for_plugin_roots(&path, harness, depth_remaining - 1, found);
        }
    }
}

/// Enumerate skills shipped by plugins in a plugin cache root
/// (`~/.claude/plugins/cache` or `~/.codex/plugins/cache`).
pub fn enumerate_plugin_skills(cache_dir: &Path, harness: &str) -> Vec<PluginSkillDir> {
    let mut out = Vec::new();
    for plugin in find_plugin_roots(cache_dir, harness, 3) {
        let skills_dir = plugin.root.join("skills");
        let Ok(entries) = fs::read_dir(&skills_dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let skill_dir = entry.path();
            if skill_dir.join("SKILL.md").is_file() {
                out.push(PluginSkillDir {
                    skill_dir,
                    plugin: plugin.info.clone(),
                });
            }
        }
    }
    out
}

/// Enumerate every plugin-shipped skill across the plugin harnesses we know
/// how to scan natively: Claude Code and Codex.
pub fn scan_plugin_skills(home: &Path) -> Vec<PluginSkillDir> {
    let mut out = enumerate_plugin_skills(&home.join(".claude/plugins/cache"), "Claude Code");
    out.extend(enumerate_plugin_skills(
        &home.join(".codex/plugins/cache"),
        "Codex",
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_plugin_cache_tree_is_enumerated() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_root = tmp
            .path()
            .join("cache/some-marketplace/sentry-toolkit/1.0.0");
        fs::create_dir_all(plugin_root.join(".claude-plugin")).unwrap();
        fs::write(
            plugin_root.join(".claude-plugin/plugin.json"),
            r#"{"name": "sentry-toolkit", "version": "1.0.0"}"#,
        )
        .unwrap();
        let skill_dir = plugin_root.join("skills/lint-code");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: lint-code\n---\n").unwrap();

        let found = enumerate_plugin_skills(&tmp.path().join("cache"), "Claude Code");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].plugin.name, "sentry-toolkit");
        assert_eq!(found[0].plugin.version.as_deref(), Some("1.0.0"));
        assert_eq!(found[0].plugin.harness, "Claude Code");
        assert_eq!(found[0].skill_dir, skill_dir);
    }

    #[test]
    fn generic_plugin_json_is_found_by_find_plugin_root() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_root = tmp.path().join("my-plugin");
        fs::create_dir_all(&plugin_root).unwrap();
        fs::write(
            plugin_root.join("plugin.json"),
            r#"{"$schema": "https://agent-plugins.org/schema.json", "name": "my-plugin", "description": "does things"}"#,
        )
        .unwrap();
        let skill_dir = plugin_root.join("skills/do-things");
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(skill_dir.join("SKILL.md"), "---\nname: do-things\n---\n").unwrap();

        let info = find_plugin_root(&skill_dir, "Codex").unwrap();
        assert_eq!(info.name, "my-plugin");
        assert_eq!(info.harness, "Codex");
        assert!(info.version.is_none());
    }

    #[test]
    fn no_manifest_yields_no_plugin_root() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("just-a-dir/skills/notes");
        fs::create_dir_all(&skill_dir).unwrap();

        assert!(find_plugin_root(&skill_dir, "Claude Code").is_none());
    }
}
