//! Scope resolution: turns `SKILL_STUDIO_HOME`, `SKILL_STUDIO_FIXTURE`, and
//! `SKILL_STUDIO_PROJECT` environment variables into a `RuntimeScope` and a
//! lease root. This mirrors `apps/cli/src/scope.rs`'s flag-driven version
//! exactly, rule for rule: an explicit home or fixture derives its own
//! history and lease roots from itself, never from the real machine's data
//! root; only the true default (neither variable set) resolves against the
//! host home lookup and the ambient `data_root()`. The core never reads the
//! environment; this module is where the MCP server does.

use std::path::PathBuf;

use skill_studio_core::scope::ProjectSelection;
use skill_studio_core::RuntimeScope;

/// Builds the `RuntimeScope` and the lease root for this process's
/// environment, one per tool call so a restart between two calls sees the
/// same rules re-applied from scratch.
pub fn resolve() -> (RuntimeScope, PathBuf) {
    let projects = project_paths();

    if let Some(fixture) = std::env::var_os("SKILL_STUDIO_FIXTURE") {
        let fixture = PathBuf::from(fixture);
        let mut scope = RuntimeScope::fixture(&fixture);
        if !projects.is_empty() {
            scope.projects = ProjectSelection::Explicit { paths: projects };
        }
        let lease_root = fixture.join(".history").join("leases");
        return (scope, lease_root);
    }

    if let Some(home) = std::env::var_os("SKILL_STUDIO_HOME") {
        let home = PathBuf::from(home);
        let data_root = home.join(".skill-studio");
        let history_root = data_root.join("history");
        let mut scope = RuntimeScope::live(home, history_root);
        if !projects.is_empty() {
            scope.projects = ProjectSelection::Explicit { paths: projects };
        }
        let lease_root = data_root.join("leases");
        return (scope, lease_root);
    }

    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
    let data_root = data_root();
    let history_root = data_root.join("history");
    let mut scope = RuntimeScope::live(home, history_root);
    if !projects.is_empty() {
        scope.projects = ProjectSelection::Explicit { paths: projects };
    }
    let lease_root = data_root.join("leases");
    (scope, lease_root)
}

/// `SKILL_STUDIO_PROJECT` is a `PATH`-style list of project directories,
/// split with the platform's path-list separator (`:` on Unix, `;` on
/// Windows) via [`std::env::split_paths`].
fn project_paths() -> Vec<PathBuf> {
    std::env::var_os("SKILL_STUDIO_PROJECT")
        .map(|value| std::env::split_paths(&value).collect())
        .unwrap_or_default()
}

/// `$XDG_DATA_HOME/skill-studio`, or `~/.local/share/skill-studio` when
/// `XDG_DATA_HOME` is unset, matching the XDG base directory spec and
/// `apps/cli/src/scope.rs::data_root`.
fn data_root() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_DATA_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join("skill-studio");
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join(".local/share/skill-studio")
}
