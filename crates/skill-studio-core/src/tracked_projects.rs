//! Tracked projects: the folders a user adds by hand or stops tracking,
//! saved once in `~/.agents/skill-studio.json` so every surface (desktop,
//! CLI, MCP server) sees the same list instead of replaying it from one
//! process's own storage.

use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::ownership::{skill_studio_json_path, OWNERSHIP_LEDGER_MAX_BYTES};
use crate::ports::ScopeFs;
use crate::scope::PhysicalRoot;

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

    /// `candidates` plus `added`, minus paths that no longer exist, paths
    /// excluded by canonical path, and the home itself.
    ///
    /// A path that fails [`ScopeFs::canonicalize`] is dropped as missing.
    /// The home is compared by canonical path when it can be canonicalized,
    /// and by the path as given otherwise - a home that can't be resolved
    /// still must not slip into the result through a lexical match. The
    /// result is sorted and deduplicated by canonical path; `lexical` keeps
    /// the path as the caller or the file gave it.
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
        let mut roots: Vec<PhysicalRoot> = candidates
            .into_iter()
            .chain(self.added.iter().cloned())
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
}
