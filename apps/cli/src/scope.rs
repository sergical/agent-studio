//! Scope resolution: turns `--fixture`/`--home`/`--project` flags into a
//! `RuntimeScope` and a lease root. The core never reads `HOME` or calls
//! `dirs`; this module is where that happens.

use std::path::{Path, PathBuf};

use clap::Args;
use skill_studio_core::scope::ProjectSelection;
use skill_studio_core::RuntimeScope;

#[derive(Args)]
pub struct ScopeArgs {
    /// Use a fixture directory as the home root instead of the real machine.
    #[arg(long)]
    fixture: Option<PathBuf>,
    /// Home root to scan, instead of the host's home directory.
    #[arg(long)]
    home: Option<PathBuf>,
    /// Explicit project directories. Without one, projects are discovered.
    #[arg(long = "project")]
    projects: Vec<PathBuf>,
    /// Shared-lease wait budget for this call, in milliseconds. Hidden: only
    /// the test suite sets this, to force a `Partial` scan deterministically.
    #[arg(long = "read-timeout-ms", hide = true)]
    read_timeout_ms: Option<u64>,
    /// Exclusive-lease wait budget for this call, in milliseconds. Hidden:
    /// only the test suite sets this, to force a fast `scope_busy` on a
    /// write command instead of waiting out the real (10s) default.
    #[arg(long = "write-timeout-ms", hide = true)]
    write_timeout_ms: Option<u64>,
}

impl ScopeArgs {
    /// Builds the `RuntimeScope` and the lease root for this invocation.
    ///
    /// `--fixture <dir>` builds `RuntimeScope::fixture(dir)`, with its lease
    /// root at `<dir>/.history/leases`. `--home <dir>` builds a `Live` scope
    /// rooted at `<dir>`, but keeps its history and lease roots namespaced
    /// under that same `<dir>` (`<dir>/.skill-studio/history` and
    /// `<dir>/.skill-studio/leases`) rather than the real machine's data
    /// directory, so an explicit `--home` never reads or writes outside the
    /// directory the caller gave us. Only the true default — neither flag
    /// given — resolves against the host's home directory and the ambient
    /// `data_root()` (`$XDG_DATA_HOME/skill-studio` or
    /// `~/.local/share/skill-studio`).
    pub fn resolve(&self) -> (RuntimeScope, PathBuf) {
        self.resolve_with_extra_project(None)
    }

    /// Like [`Self::resolve`], but folds `extra_project` into the scope's
    /// explicit project list even when `--project` was never given.
    /// `add --project-path` installs into a project the scope has not
    /// otherwise been told about; without this, the runtime's `ScopeFs`
    /// rejects the write as outside the scope ("path lies outside the
    /// scope") because `scope.projects` never named it.
    pub fn resolve_with_extra_project(
        &self,
        extra_project: Option<&Path>,
    ) -> (RuntimeScope, PathBuf) {
        let projects: Vec<PathBuf> = self
            .projects
            .iter()
            .cloned()
            .chain(extra_project.map(Path::to_path_buf))
            .collect();
        let (mut scope, lease_root) = if let Some(fixture) = &self.fixture {
            let mut scope = RuntimeScope::fixture(fixture);
            if !projects.is_empty() {
                scope.projects = ProjectSelection::Explicit {
                    paths: projects.clone(),
                };
            }
            scope.opencode_config_root =
                Some(skill_studio_host::opencode_config_dir_under(fixture));
            let lease_root = fixture.join(".history").join("leases");
            (scope, lease_root)
        } else if let Some(home) = &self.home {
            let data_root = home.join(".skill-studio");
            let history_root = data_root.join("history");
            let codex_home = skill_studio_host::codex_home(home);
            let mut scope =
                RuntimeScope::live(home.clone(), history_root).with_codex_home(codex_home);
            if !projects.is_empty() {
                scope.projects = ProjectSelection::Explicit {
                    paths: projects.clone(),
                };
            }
            scope.opencode_config_root = Some(skill_studio_host::opencode_config_dir_under(home));
            let lease_root = data_root.join("leases");
            (scope, lease_root)
        } else {
            let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
            let data_root = data_root();
            let history_root = data_root.join("history");
            let codex_home = skill_studio_host::codex_home(&home);
            let mut scope =
                RuntimeScope::live(home.clone(), history_root).with_codex_home(codex_home);
            if !projects.is_empty() {
                scope.projects = ProjectSelection::Explicit {
                    paths: projects.clone(),
                };
            }
            scope.opencode_config_root = Some(skill_studio_host::opencode_config_dir(&home));
            let lease_root = data_root.join("leases");
            (scope, lease_root)
        };
        if let Some(ms) = self.read_timeout_ms {
            scope.read_timeout_ms = ms;
        }
        if let Some(ms) = self.write_timeout_ms {
            scope.write_timeout_ms = ms;
        }
        (scope, lease_root)
    }
}

/// The desktop app's own `timing.jsonl`: `<dirs::data_dir()>/com.skillstudio.app/timing.jsonl`
/// (`~/Library/Application Support/com.skillstudio.app/timing.jsonl` on
/// macOS), matching Tauri's `app_data_dir()` resolution for the
/// `identifier` in `apps/desktop/src-tauri/tauri.conf.json`. `health`'s
/// default when `--timing-log` is not given; the CLI has no other notion of
/// an app data dir, so this resolves it through `dirs` directly rather than
/// adding one.
pub fn default_timing_log_path() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("/"))
        .join("com.skillstudio.app")
        .join("timing.jsonl")
}

/// `$XDG_DATA_HOME/skill-studio`, or `~/.local/share/skill-studio` when
/// `XDG_DATA_HOME` is unset, matching the XDG base directory spec.
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
