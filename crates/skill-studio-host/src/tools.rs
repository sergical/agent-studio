//! [`ToolLookup`] over the real `PATH`.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use skill_studio_core::ports::ToolLookup;

/// `ToolLookup` backed by the process `PATH`.
///
/// Unix semantics only: a name resolves when a directory on the search path
/// holds a regular file with any executable bit set. There is no `PATHEXT`
/// step and no extension guessing.
pub struct PathToolLookup {
    /// Directories searched in order, most preferred first.
    search_dirs: Vec<PathBuf>,
}

impl PathToolLookup {
    /// Builds a lookup over the current process's `PATH` environment
    /// variable. An unset or empty `PATH` searches nothing.
    pub fn new() -> Self {
        let path = std::env::var_os("PATH").unwrap_or_default();
        PathToolLookup {
            search_dirs: std::env::split_paths(&path).collect(),
        }
    }

    /// Builds a lookup over an explicit list of directories, most preferred
    /// first. Intended for tests, which do not want to depend on the real
    /// `PATH`.
    pub fn with_search_dirs(search_dirs: Vec<PathBuf>) -> Self {
        PathToolLookup { search_dirs }
    }
}

impl Default for PathToolLookup {
    fn default() -> Self {
        PathToolLookup::new()
    }
}

/// True when `path` names a regular file with an executable bit set for the
/// owner, group, or others.
fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

impl ToolLookup for PathToolLookup {
    fn find_binary(&self, name: &str) -> Option<PathBuf> {
        self.search_dirs.iter().find_map(|dir| {
            let candidate = dir.join(name);
            is_executable_file(&candidate)
                .then(|| std::fs::canonicalize(&candidate).unwrap_or(candidate))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_executable(path: &Path) {
        fs::write(path, b"#!/bin/sh\nexit 0\n").unwrap();
        let mut perms = fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).unwrap();
    }

    #[test]
    fn finds_an_executable_on_the_search_path() {
        let tmp = tempfile::tempdir().unwrap();
        let bin = tmp.path().join("my-tool");
        write_executable(&bin);

        let lookup = PathToolLookup::with_search_dirs(vec![tmp.path().to_path_buf()]);
        let found = lookup.find_binary("my-tool").unwrap();
        assert_eq!(found, fs::canonicalize(&bin).unwrap());
    }

    #[test]
    fn ignores_a_non_executable_file_with_the_same_name() {
        let tmp = tempfile::tempdir().unwrap();
        let not_a_tool = tmp.path().join("not-a-tool");
        fs::write(&not_a_tool, b"plain text").unwrap();
        let mut perms = fs::metadata(&not_a_tool).unwrap().permissions();
        perms.set_mode(0o644);
        fs::set_permissions(&not_a_tool, perms).unwrap();

        let lookup = PathToolLookup::with_search_dirs(vec![tmp.path().to_path_buf()]);
        assert!(lookup.find_binary("not-a-tool").is_none());
    }

    #[test]
    fn returns_none_when_no_search_dir_has_the_name() {
        let tmp = tempfile::tempdir().unwrap();
        let lookup = PathToolLookup::with_search_dirs(vec![tmp.path().to_path_buf()]);
        assert!(lookup.find_binary("does-not-exist").is_none());
    }

    #[test]
    fn stops_at_the_first_match_in_search_order() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        write_executable(&first.path().join("tool"));
        write_executable(&second.path().join("tool"));

        let lookup = PathToolLookup::with_search_dirs(vec![
            first.path().to_path_buf(),
            second.path().to_path_buf(),
        ]);
        let found = lookup.find_binary("tool").unwrap();
        assert_eq!(found, fs::canonicalize(first.path().join("tool")).unwrap());
    }
}
