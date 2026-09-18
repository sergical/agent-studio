//! Tracked projects: the folders a user adds by hand or stops tracking,
//! saved once in `~/.agents/skill-studio.json` so every surface (desktop,
//! CLI, MCP server) sees the same list instead of replaying it from one
//! process's own storage.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ownership::{skill_studio_json_path, OWNERSHIP_LEDGER_MAX_BYTES};
use crate::ports::{FileKind, ScopeFs};
use crate::scope::PhysicalRoot;

/// Skill directories (relative to a project root) whose presence marks a
/// directory as a real skills project, not just any directory a session
/// happened to run in or a folder a glob pattern happened to match.
pub const SKILL_DIR_MARKERS: &[&str] = &[
    ".claude/skills",
    ".codex/skills",
    ".opencode/skills",
    ".opencode/skill",
    ".pi/skills",
    ".cursor/skills",
    ".grok/skills",
    ".agents/skills",
];

/// Expands a leading `~` (or a bare `~`) against `home`; any other path is
/// returned unchanged.
pub fn expand_home(path: &Path, home: &Path) -> PathBuf {
    if path == Path::new("~") {
        return home.to_path_buf();
    }
    match path.strip_prefix("~/") {
        Ok(rest) => home.join(rest),
        Err(_) => path.to_path_buf(),
    }
}

/// True when `path`'s last component is exactly `*`.
pub fn is_pattern(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == "*")
}

/// The folders a `*`-suffixed [`TrackedProjects::added`] entry expands to,
/// and whether its parent folder could even be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternMatches {
    /// The pattern exactly as saved (e.g. `~/src/*`).
    pub pattern: PathBuf,
    /// Whether the parent folder (after `~` expansion) could be listed.
    pub parent_found: bool,
    /// The matching child folders, expanded and lexical.
    pub folders: Vec<PathBuf>,
}

/// Lists the parent of `pattern` (after `~` expansion) and keeps the
/// directory and symlink children that hold at least one
/// [`SKILL_DIR_MARKERS`] entry. Hidden children (a name starting with `.`)
/// are skipped. Children are re-read on every call, so a folder created
/// after the pattern was saved appears on the next call and a deleted one
/// disappears.
fn expand_pattern(fs: &dyn ScopeFs, home: &Path, pattern: &Path) -> PatternMatches {
    let expanded = expand_home(pattern, home);
    let parent = expanded.parent().unwrap_or(&expanded).to_path_buf();
    let Ok(entries) = fs.read_dir(&parent) else {
        return PatternMatches {
            pattern: pattern.to_path_buf(),
            parent_found: false,
            folders: Vec::new(),
        };
    };
    let folders = entries
        .into_iter()
        .filter(|entry| matches!(entry.kind, FileKind::Dir | FileKind::Symlink))
        .filter(|entry| !entry.name.starts_with('.'))
        .map(|entry| parent.join(&entry.name))
        .filter(|child| {
            SKILL_DIR_MARKERS
                .iter()
                .any(|marker| fs.canonicalize(&child.join(marker)).is_ok())
        })
        .collect();
    PatternMatches {
        pattern: pattern.to_path_buf(),
        parent_found: true,
        folders,
    }
}

/// Validates a freshly typed `added` entry and returns the value to store,
/// or a plain, user-facing error.
///
/// A pattern (last component exactly `*`) is stored exactly as typed, with
/// its `~` left unexpanded; a plain path is expanded and stored absolute so
/// it matches the canonical path a resolved row shows for "Remove". Both
/// forms must exist as a directory (a pattern's parent, a plain path
/// itself) before they are accepted.
pub fn entry_to_save(fs: &dyn ScopeFs, home: &Path, typed: &str) -> Result<PathBuf, String> {
    let trimmed = typed.trim();
    if trimmed.is_empty() {
        return Err("Use a full path, such as ~/src/app or /Users/you/src/app.".to_string());
    }
    let typed_path = Path::new(trimmed);
    let starts_right = trimmed.starts_with('/') || trimmed.starts_with("~/") || trimmed == "~";
    if !starts_right {
        return Err("Use a full path, such as ~/src/app or /Users/you/src/app.".to_string());
    }

    let components: Vec<_> = typed_path.components().collect();
    let last_is_pattern = is_pattern(typed_path);
    let has_stray_star = components.iter().enumerate().any(|(i, c)| {
        let is_last = i + 1 == components.len();
        let text = c.as_os_str().to_string_lossy();
        text.contains('*') && !(is_last && text == "*")
    });
    if has_stray_star {
        return Err("Only the last part of a path can be *, as in ~/src/*.".to_string());
    }

    if last_is_pattern {
        // Report the parent as typed (e.g. "~/nope"), not expanded, matching the plain-path
        // error below.
        let typed_parent = typed_path.parent().unwrap_or(typed_path);
        let expanded_parent = expand_home(typed_parent, home);
        if !is_existing_dir(fs, &expanded_parent) {
            return Err(format!("No folder at {}.", typed_parent.display()));
        }
        Ok(PathBuf::from(trimmed))
    } else {
        let expanded = expand_home(typed_path, home);
        if !is_existing_dir(fs, &expanded) {
            return Err(format!("No folder at {trimmed}."));
        }
        if let (Ok(canonical_home), Ok(canonical_expanded)) =
            (fs.canonicalize(home), fs.canonicalize(&expanded))
        {
            if canonical_home == canonical_expanded {
                return Err(
                    "Your home folder can't be a project folder. Add a folder inside it, or use ~/*."
                        .to_string(),
                );
            }
        }
        Ok(expanded)
    }
}

/// True when `path` exists and is a directory, following symlinks.
fn is_existing_dir(fs: &dyn ScopeFs, path: &Path) -> bool {
    fs.canonicalize(path)
        .and_then(|canonical| fs.symlink_metadata(&canonical))
        .is_ok_and(|facts| facts.kind == FileKind::Dir)
}

/// Folders added or excluded from project discovery, as recorded under the
/// `projects` key of `~/.agents/skill-studio.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TrackedProjects {
    /// Folders discovery should cover even when it would not find them on
    /// its own.
    #[serde(default)]
    pub added: Vec<PathBuf>,
    /// Folders discovery should skip even when it would otherwise find
    /// them.
    #[serde(default)]
    pub excluded: Vec<PathBuf>,
}

/// Only the `projects` key [`TrackedProjects::read`] needs; every other
/// `skill-studio.json` field (forks, copies, trials, ...) is out of scope
/// here.
#[derive(Debug, Deserialize, Default)]
struct RawSkillStudioJson {
    #[serde(default)]
    projects: TrackedProjects,
}

impl TrackedProjects {
    /// Reads `<home>/.agents/skill-studio.json`'s `projects` key.
    ///
    /// A missing, unreadable, oversize, or malformed file - or a `projects`
    /// section that doesn't match this shape - yields an empty
    /// [`TrackedProjects`] rather than an error: most homes have never
    /// tracked or excluded a folder, and callers that discover projects
    /// must still run.
    pub fn read(fs: &dyn ScopeFs, home: &Path) -> Self {
        let path = skill_studio_json_path(home);
        let Ok(bytes) = fs.read_capped(&path, OWNERSHIP_LEDGER_MAX_BYTES) else {
            return Self::default();
        };
        let Ok(raw) = serde_json::from_slice::<RawSkillStudioJson>(&bytes) else {
            return Self::default();
        };
        raw.projects
    }

    /// True when neither list has an entry.
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.excluded.is_empty()
    }

    /// Adds each path to `added`, unless it is already there, and removes
    /// it from `excluded` - re-tracking a folder undoes an earlier
    /// "stop tracking".
    pub fn track(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for path in paths {
            self.excluded.retain(|p| p != &path);
            if !self.added.contains(&path) {
                self.added.push(path);
            }
        }
    }

    /// Removes `path` from `added` and adds it to `excluded`, unless it is
    /// already there.
    pub fn untrack(&mut self, path: &Path) {
        self.added.retain(|p| p != path);
        if !self.excluded.iter().any(|p| p == path) {
            self.excluded.push(path.to_path_buf());
        }
    }

    /// Removes `path` from `added` only. Unlike [`Self::untrack`], this
    /// records no exclusion, so discovery can still find the folder later -
    /// for a folder the user added by hand and now wants gone, not one they
    /// want discovery to stop offering.
    pub fn forget(&mut self, path: &Path) {
        self.added.retain(|p| p != path);
    }

    /// `candidates` plus `added`, minus paths that no longer exist, paths
    /// excluded by canonical path, and the home itself.
    ///
    /// A `*`-suffixed `added` entry is replaced by the folders it currently
    /// matches ([`Self::pattern_matches`]) before the rest of the pipeline
    /// runs; a plain `added` entry starting with `~` is expanded against
    /// `home` first (for a hand-edited file). A path that fails
    /// [`ScopeFs::canonicalize`] is dropped as missing. The home is compared
    /// by canonical path when it can be canonicalized, and by the path as
    /// given otherwise - a home that can't be resolved still must not slip
    /// into the result through a lexical match. The result is sorted and
    /// deduplicated by canonical path; `lexical` keeps the path as the
    /// caller or the file gave it.
    pub fn resolve(
        &self,
        fs: &dyn ScopeFs,
        home: &Path,
        candidates: impl IntoIterator<Item = PathBuf>,
    ) -> Vec<PhysicalRoot> {
        let excluded: Vec<PathBuf> = self
            .excluded
            .iter()
            .filter_map(|p| fs.canonicalize(p).ok())
            .collect();
        let home_canonical = fs.canonicalize(home).ok();
        let added_expanded = self.added.iter().flat_map(|added| {
            if is_pattern(added) {
                expand_pattern(fs, home, added).folders
            } else {
                vec![expand_home(added, home)]
            }
        });
        let mut roots: Vec<PhysicalRoot> = candidates
            .into_iter()
            .chain(added_expanded)
            .filter_map(|lexical| {
                let canonical = fs.canonicalize(&lexical).ok()?;
                Some(PhysicalRoot { lexical, canonical })
            })
            .filter(|root| !excluded.contains(&root.canonical))
            .filter(|root| match &home_canonical {
                Some(home_canonical) => root.canonical != *home_canonical,
                None => root.lexical != home,
            })
            .collect();
        roots.sort_by(|a, b| a.canonical.cmp(&b.canonical));
        roots.dedup_by(|a, b| a.canonical == b.canonical);
        roots
    }

    /// The current expansion of every `*`-suffixed `added` entry, in
    /// `added` order.
    pub fn pattern_matches(&self, fs: &dyn ScopeFs, home: &Path) -> Vec<PatternMatches> {
        self.added
            .iter()
            .filter(|added| is_pattern(added))
            .map(|pattern| expand_pattern(fs, home, pattern))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureBuilder;

    #[test]
    fn read_yields_empty_for_a_missing_file() {
        let fs = FixtureBuilder::new().dir("/home/u").build_fs();
        let projects = TrackedProjects::read(&fs, Path::new("/home/u"));
        assert!(projects.is_empty());
    }

    #[test]
    fn read_yields_empty_for_malformed_json() {
        let fs = FixtureBuilder::new()
            .file("/home/u/.agents/skill-studio.json", b"not json")
            .build_fs();
        let projects = TrackedProjects::read(&fs, Path::new("/home/u"));
        assert!(projects.is_empty());
    }

    #[test]
    fn read_yields_empty_when_projects_is_absent() {
        let fs = FixtureBuilder::new()
            .file(
                "/home/u/.agents/skill-studio.json",
                br#"{"forks":{},"preferred_editor":"vscode"}"#,
            )
            .build_fs();
        let projects = TrackedProjects::read(&fs, Path::new("/home/u"));
        assert!(projects.is_empty());
    }

    #[test]
    fn read_ignores_other_keys_and_finds_projects() {
        let fs = FixtureBuilder::new()
            .file(
                "/home/u/.agents/skill-studio.json",
                br#"{"forks":{},"projects":{"added":["/home/u/a"],"excluded":["/home/u/b"]}}"#,
            )
            .build_fs();
        let projects = TrackedProjects::read(&fs, Path::new("/home/u"));
        assert_eq!(projects.added, [PathBuf::from("/home/u/a")]);
        assert_eq!(projects.excluded, [PathBuf::from("/home/u/b")]);
    }

    #[test]
    fn track_has_no_duplicates_and_unexcludes() {
        let mut projects = TrackedProjects {
            excluded: vec![PathBuf::from("/home/u/a")],
            ..Default::default()
        };
        projects.track([PathBuf::from("/home/u/a"), PathBuf::from("/home/u/a")]);
        assert_eq!(projects.added, [PathBuf::from("/home/u/a")]);
        assert!(projects.excluded.is_empty());
    }

    #[test]
    fn untrack_excludes_once() {
        let mut projects = TrackedProjects {
            added: vec![PathBuf::from("/home/u/a")],
            ..Default::default()
        };
        projects.untrack(Path::new("/home/u/a"));
        projects.untrack(Path::new("/home/u/a"));
        assert!(projects.added.is_empty());
        assert_eq!(projects.excluded, [PathBuf::from("/home/u/a")]);
    }

    #[test]
    fn forget_removes_from_added_without_excluding() {
        let mut projects = TrackedProjects {
            added: vec![PathBuf::from("/home/u/a")],
            ..Default::default()
        };
        projects.forget(Path::new("/home/u/a"));
        assert!(projects.added.is_empty());
        assert!(projects.excluded.is_empty());
    }

    #[test]
    fn resolve_unions_candidates_and_added_dropping_excluded_and_missing() {
        let fs = FixtureBuilder::new()
            .dir("/home/u")
            .dir("/home/u/kept")
            .dir("/home/u/also-kept")
            .dir("/home/u/dropped")
            .build_fs();
        let projects = TrackedProjects {
            added: vec![PathBuf::from("/home/u/also-kept")],
            excluded: vec![PathBuf::from("/home/u/dropped")],
        };
        let found = projects.resolve(
            &fs,
            Path::new("/home/u"),
            [
                PathBuf::from("/home/u/kept"),
                PathBuf::from("/home/u/dropped"),
                PathBuf::from("/home/u/missing"),
            ],
        );
        let lexical: Vec<_> = found.iter().map(|p| p.lexical.as_path()).collect();
        assert_eq!(
            lexical,
            [Path::new("/home/u/also-kept"), Path::new("/home/u/kept"),]
        );
    }

    #[test]
    fn resolve_drops_the_home_from_either_source() {
        let fs = FixtureBuilder::new()
            .dir("/home/u")
            .dir("/home/u/kept")
            .build_fs();
        let projects = TrackedProjects {
            added: vec![PathBuf::from("/home/u")],
            ..Default::default()
        };
        let found = projects.resolve(
            &fs,
            Path::new("/home/u"),
            [PathBuf::from("/home/u"), PathBuf::from("/home/u/kept")],
        );
        let lexical: Vec<_> = found.iter().map(|p| p.lexical.as_path()).collect();
        assert_eq!(lexical, [Path::new("/home/u/kept")]);
    }

    #[test]
    fn resolve_excludes_by_canonical_path_through_an_alias() {
        let fs = FixtureBuilder::new()
            .dir("/vol/real")
            .dir("/vol/real/proj")
            .alias("/home/u", "/vol/real")
            .build_fs();
        let projects = TrackedProjects {
            excluded: vec![PathBuf::from("/home/u/proj")],
            ..Default::default()
        };
        let found = projects.resolve(&fs, Path::new("/home/u"), [PathBuf::from("/vol/real/proj")]);
        assert!(found.is_empty());
    }

    #[test]
    fn resolve_sorts_and_dedupes_by_canonical_path() {
        let fs = FixtureBuilder::new()
            .dir("/home/u")
            .dir("/home/u/b")
            .dir("/home/u/a")
            .build_fs();
        let projects = TrackedProjects {
            added: vec![PathBuf::from("/home/u/a")],
            ..Default::default()
        };
        let found = projects.resolve(
            &fs,
            Path::new("/home/u"),
            [PathBuf::from("/home/u/b"), PathBuf::from("/home/u/a")],
        );
        let lexical: Vec<_> = found.iter().map(|p| p.lexical.as_path()).collect();
        assert_eq!(lexical, [Path::new("/home/u/a"), Path::new("/home/u/b")]);
    }

    #[test]
    fn resolve_expands_a_pattern_to_its_skill_folders() {
        let fs = FixtureBuilder::new()
            .dir("/home/u")
            .dir("/home/u/src/a/.claude/skills/x")
            .file("/home/u/src/a/.claude/skills/x/SKILL.md", b"")
            .dir("/home/u/src/b")
            .dir("/home/u/src/.hidden/.claude/skills/y")
            .file("/home/u/src/.hidden/.claude/skills/y/SKILL.md", b"")
            .build_fs();
        let projects = TrackedProjects {
            added: vec![PathBuf::from("~/src/*")],
            ..Default::default()
        };
        let found = projects.resolve(&fs, Path::new("/home/u"), []);
        let lexical: Vec<_> = found.iter().map(|p| p.lexical.as_path()).collect();
        assert_eq!(lexical, [Path::new("/home/u/src/a")]);
    }

    #[test]
    fn resolve_picks_up_a_folder_added_after_the_pattern_was_saved() {
        let fs = FixtureBuilder::new()
            .dir("/home/u")
            .dir("/home/u/src/a/.claude/skills/x")
            .file("/home/u/src/a/.claude/skills/x/SKILL.md", b"")
            .dir("/home/u/src/b")
            .dir("/home/u/src/c/.agents/skills/z")
            .file("/home/u/src/c/.agents/skills/z/SKILL.md", b"")
            .build_fs();
        let projects = TrackedProjects {
            added: vec![PathBuf::from("~/src/*")],
            ..Default::default()
        };
        let found = projects.resolve(&fs, Path::new("/home/u"), []);
        let lexical: Vec<_> = found.iter().map(|p| p.lexical.as_path()).collect();
        assert_eq!(
            lexical,
            [Path::new("/home/u/src/a"), Path::new("/home/u/src/c")]
        );
    }

    #[test]
    fn resolve_drops_an_excluded_pattern_match_and_dedupes_a_discovered_one() {
        let fs = FixtureBuilder::new()
            .dir("/home/u")
            .dir("/home/u/src/a/.claude/skills/x")
            .file("/home/u/src/a/.claude/skills/x/SKILL.md", b"")
            .dir("/home/u/src/b/.claude/skills/x")
            .file("/home/u/src/b/.claude/skills/x/SKILL.md", b"")
            .build_fs();
        let projects = TrackedProjects {
            added: vec![PathBuf::from("~/src/*")],
            excluded: vec![PathBuf::from("/home/u/src/a")],
        };
        let found = projects.resolve(&fs, Path::new("/home/u"), [PathBuf::from("/home/u/src/b")]);
        let lexical: Vec<_> = found.iter().map(|p| p.lexical.as_path()).collect();
        assert_eq!(lexical, [Path::new("/home/u/src/b")]);
    }

    #[test]
    fn entry_to_save_validates_and_normalizes() {
        let fs = FixtureBuilder::new()
            .dir("/home/u")
            .dir("/home/u/src/a")
            .build_fs();
        let home = Path::new("/home/u");

        assert_eq!(
            entry_to_save(&fs, home, "~/src/*").unwrap(),
            PathBuf::from("~/src/*")
        );
        assert_eq!(
            entry_to_save(&fs, home, "~/src/a").unwrap(),
            PathBuf::from("/home/u/src/a")
        );
        assert!(entry_to_save(&fs, home, "~/*/x").is_err());
        assert!(entry_to_save(&fs, home, "~/src/app-*").is_err());
        assert!(entry_to_save(&fs, home, "src/*").is_err());
        assert!(entry_to_save(&fs, home, "").is_err());
        assert_eq!(
            entry_to_save(&fs, home, "~/nope/*").unwrap_err(),
            "No folder at ~/nope."
        );
        assert!(entry_to_save(&fs, home, "~/nope").is_err());
        assert_eq!(
            entry_to_save(&fs, home, "~").unwrap_err(),
            "Your home folder can't be a project folder. Add a folder inside it, or use ~/*."
        );
        assert_eq!(
            entry_to_save(&fs, home, "/home/u").unwrap_err(),
            "Your home folder can't be a project folder. Add a folder inside it, or use ~/*."
        );
    }

    #[test]
    fn pattern_matches_reports_a_missing_parent() {
        let fs = FixtureBuilder::new().dir("/home/u").build_fs();
        let projects = TrackedProjects {
            added: vec![PathBuf::from("~/nope/*")],
            ..Default::default()
        };
        let matches = projects.pattern_matches(&fs, Path::new("/home/u"));
        assert_eq!(matches.len(), 1);
        assert!(!matches[0].parent_found);
        assert!(matches[0].folders.is_empty());

        let found = projects.resolve(&fs, Path::new("/home/u"), []);
        assert!(found.is_empty());
    }
}
