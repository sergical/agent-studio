//! Runtime scope: the explicit set of roots one operation may touch.
//!
//! The core never guesses a home directory. An adapter builds a
//! [`RuntimeScope`], the core normalizes it once into a [`NormalizedScope`],
//! and every port call and every id derives from that normalized value.

use std::path::{Path, PathBuf};
use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ErrorCode};
use crate::identity::sha256_hex;
use crate::ports::{LeaseKey, ProjectDiscovery, ScopeFs};
use crate::tracked_projects::TrackedProjects;

/// Default wait for a shared lease on a read operation.
pub const DEFAULT_READ_TIMEOUT_MS: u64 = 2_000;
/// Default wait for an exclusive lease on a write operation.
pub const DEFAULT_WRITE_TIMEOUT_MS: u64 = 10_000;

/// Whether the scope points at a real user home or a test fixture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    /// A real user home. History overrides are refused.
    Live,
    /// A fixture built by tests or by `--fixture`. Overrides are allowed.
    Fixture,
}

/// Which projects an operation covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "mode")]
pub enum ProjectSelection {
    /// Exactly these project directories, in any order.
    Explicit {
        /// Absolute project paths.
        paths: Vec<PathBuf>,
    },
    /// Discover projects from harness session stores, minus `exclude`.
    Discover {
        /// Absolute paths to skip even when discovered.
        exclude: Vec<PathBuf>,
    },
}

/// How `history_root` was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HistoryBinding {
    /// The adapter's platform default (app data dir or XDG data dir).
    Default,
    /// A caller-supplied path. Valid only with [`ScopeKind::Fixture`].
    Override,
}

/// Adapter-supplied description of the roots one call may read or write.
///
/// Invariant: every path is absolute. The core reads no environment variable
/// to fill a missing field; a missing field is an [`ErrorCode::InvalidScope`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeScope {
    /// Live or fixture.
    pub kind: ScopeKind,
    /// The directory that plays the role of `~`.
    pub home_root: PathBuf,
    /// Projects to cover.
    pub projects: ProjectSelection,
    /// Directory that holds `events.sqlite3` and `backups/`.
    pub history_root: PathBuf,
    /// Where `history_root` came from.
    pub history_binding: HistoryBinding,
    /// Directory for caches. `None` means no cache is written.
    pub cache_root: Option<PathBuf>,
    /// Directory for durable app data that is neither history nor cache:
    /// fork base snapshots (`forks/`), the update-check store
    /// (`update-check.json`), run history (`runs/`), and the invocation
    /// cache. `None` means the operations that need it fail with
    /// [`ErrorCode::InvalidScope`]. Phase 1 and 2 operations never read it.
    pub data_root: Option<PathBuf>,
    /// Directory OpenCode's own `opencode.json`/`opencode.jsonc` lives in.
    /// `None` means the plain default, `<home_root>/.config/opencode`: an
    /// adapter that wants `XDG_CONFIG_HOME`/`OPENCODE_CONFIG_DIR` honored
    /// resolves them itself (see the host's `opencode_config_dir`) and sets
    /// this field, since the core reads no environment variable. Read by
    /// [`crate::ops::scan`] for OpenCode's native per-skill deny switch.
    pub opencode_config_root: Option<PathBuf>,
    /// Codex's own directory, in place of `home_root/.codex`. `None` keeps
    /// the default. The adapter is the one place allowed to read
    /// `CODEX_HOME`; the core only ever sees the resolved path here (see
    /// `docs/action-map/harnesses/codex.md`, "`CODEX_HOME` overrides
    /// `~/.codex`").
    pub codex_home: Option<PathBuf>,
    /// Shared-lease wait budget for read operations, in milliseconds.
    pub read_timeout_ms: u64,
    /// Exclusive-lease wait budget for write operations, in milliseconds.
    pub write_timeout_ms: u64,
}

impl RuntimeScope {
    /// Builds a live scope with default timeouts and discovered projects.
    pub fn live(home_root: impl Into<PathBuf>, history_root: impl Into<PathBuf>) -> Self {
        RuntimeScope {
            kind: ScopeKind::Live,
            home_root: home_root.into(),
            projects: ProjectSelection::Discover {
                exclude: Vec::new(),
            },
            history_root: history_root.into(),
            history_binding: HistoryBinding::Default,
            cache_root: None,
            data_root: None,
            opencode_config_root: None,
            codex_home: None,
            read_timeout_ms: DEFAULT_READ_TIMEOUT_MS,
            write_timeout_ms: DEFAULT_WRITE_TIMEOUT_MS,
        }
    }

    /// Builds a fixture scope whose history lives under `home_root/.history`
    /// and whose app data lives under `home_root/.history/data`.
    pub fn fixture(home_root: impl Into<PathBuf>) -> Self {
        let home_root = home_root.into();
        let history_root = home_root.join(".history");
        let data_root = history_root.join("data");
        RuntimeScope {
            kind: ScopeKind::Fixture,
            home_root,
            projects: ProjectSelection::Explicit { paths: Vec::new() },
            history_root,
            history_binding: HistoryBinding::Override,
            cache_root: None,
            data_root: Some(data_root),
            opencode_config_root: None,
            codex_home: None,
            read_timeout_ms: DEFAULT_READ_TIMEOUT_MS,
            write_timeout_ms: DEFAULT_WRITE_TIMEOUT_MS,
        }
    }

    /// Overrides Codex's own directory, in place of `home_root/.codex`. The
    /// adapter calls this after reading `CODEX_HOME` itself - the core never
    /// reads it.
    #[must_use]
    pub fn with_codex_home(mut self, codex_home: impl Into<PathBuf>) -> Self {
        self.codex_home = Some(codex_home.into());
        self
    }

    /// Codex's own directory: the override, or `home_root/.codex`.
    pub fn codex_home_or_default(&self) -> PathBuf {
        self.codex_home
            .clone()
            .unwrap_or_else(|| self.home_root.join(".codex"))
    }

    /// Read wait budget as a duration.
    pub fn read_timeout(&self) -> Duration {
        Duration::from_millis(self.read_timeout_ms)
    }

    /// Write wait budget as a duration.
    pub fn write_timeout(&self) -> Duration {
        Duration::from_millis(self.write_timeout_ms)
    }
}

/// Stable identity of a scope.
///
/// Invariant: `scope:v1/<sha256 of the canonical home path>`. Two scopes
/// that name the same physical home through different aliases share one id.
/// Projects do not take part, so adding a project keeps the id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct ScopeId(String);

impl ScopeId {
    /// Prefix every id carries.
    pub const PREFIX: &'static str = "scope:v1/";

    /// Derives the id from a home path. `canonical` need not actually be
    /// canonicalized: an adapter that failed to normalize a scope (so has no
    /// [`PhysicalRoot`] yet) still needs an id for the error envelope's
    /// [`EffectiveScope`], and a lexical path is the best it has.
    pub fn for_canonical_home(canonical: &Path) -> Self {
        let hex = sha256_hex(canonical.to_string_lossy().as_bytes());
        ScopeId(format!("{}{}", Self::PREFIX, hex))
    }

    /// Returns the wire string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A root with both the name the caller used and the place the bytes live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PhysicalRoot {
    /// Path as the caller gave it.
    pub lexical: PathBuf,
    /// Path after every symlink is resolved.
    pub canonical: PathBuf,
}

/// A scope after normalization.
///
/// Invariant: `projects` is sorted by canonical path and has no duplicates.
/// Lease keys, ids, and golden snapshots derive from canonical paths only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NormalizedScope {
    /// Stable identity.
    pub id: ScopeId,
    /// The home root.
    pub home: PhysicalRoot,
    /// Projects the scope covers. Under [`ProjectSelection::Discover`] these
    /// come from the [`ProjectDiscovery`] port plus the added folders saved
    /// in `~/.agents/skill-studio.json` at normalization time; the list
    /// never grows afterwards, so lease keys and `contains` are fixed for
    /// the life of the runtime.
    pub projects: Vec<PhysicalRoot>,
    /// Directory that holds `events.sqlite3` and `backups/`.
    pub history_root: PathBuf,
    /// Cache directory, when caches are allowed.
    pub cache_root: Option<PathBuf>,
    /// Durable app data directory, when the adapter supplies one.
    pub data_root: Option<PathBuf>,
    /// Codex's own directory (the resolved `CODEX_HOME`, or
    /// `home_root/.codex`). Lexical only: unlike `home` and `projects` it is
    /// not required to exist yet, since a config write is often what first
    /// creates it.
    pub codex_home: PathBuf,
    /// The scope as the adapter gave it.
    pub raw: RuntimeScope,
}

impl NormalizedScope {
    /// Resolves aliases and validates the scope without a discovery port.
    ///
    /// Fails with [`ErrorCode::InvalidScope`] when the home is relative or
    /// missing, when a history override is used outside fixture mode, when
    /// a project is the home itself, or when `projects` is
    /// [`ProjectSelection::Discover`] (that mode needs
    /// [`Self::normalize_with_discovery`]).
    pub fn normalize(raw: &RuntimeScope, fs: &dyn ScopeFs) -> Result<Self, CoreError> {
        Self::normalize_with_discovery(raw, fs, None)
    }

    /// Resolves aliases and validates the scope, discovering projects
    /// through `discovery` under [`ProjectSelection::Discover`].
    ///
    /// Discovered candidates are joined with the folders the user added by
    /// hand, recorded in `~/.agents/skill-studio.json`'s `projects.added`
    /// (see [`crate::tracked_projects::TrackedProjects`]). Candidates that
    /// no longer exist are dropped; paths in `projects.excluded` or in
    /// `exclude` are removed by canonical path. The home is never a
    /// project - here it is silently dropped rather than an error.
    pub fn normalize_with_discovery(
        raw: &RuntimeScope,
        fs: &dyn ScopeFs,
        discovery: Option<&dyn ProjectDiscovery>,
    ) -> Result<Self, CoreError> {
        if !raw.home_root.is_absolute() {
            return Err(CoreError::new(
                ErrorCode::InvalidScope,
                "home_root must be an absolute path",
            )
            .at(&raw.home_root));
        }
        if raw.history_binding == HistoryBinding::Override && raw.kind != ScopeKind::Fixture {
            return Err(CoreError::new(
                ErrorCode::InvalidScope,
                "a history override is allowed only in fixture mode",
            )
            .at(&raw.history_root));
        }
        let home = physical(fs, &raw.home_root)?;
        let mut projects = match &raw.projects {
            ProjectSelection::Explicit { paths } => paths
                .iter()
                .map(|p| physical(fs, p))
                .collect::<Result<Vec<_>, _>>()?,
            ProjectSelection::Discover { exclude } => {
                let Some(discovery) = discovery else {
                    return Err(CoreError::new(
                        ErrorCode::InvalidScope,
                        "project discovery needs a ProjectDiscovery port; pass explicit projects",
                    )
                    .at(&raw.home_root));
                };
                let mut tracked = TrackedProjects::read(fs, &raw.home_root);
                tracked.excluded.extend(exclude.iter().cloned());
                tracked.resolve(
                    fs,
                    &raw.home_root,
                    discovery.discover_projects(&raw.home_root)?,
                )
            }
        };
        projects.sort_by(|a, b| a.canonical.cmp(&b.canonical));
        projects.dedup_by(|a, b| a.canonical == b.canonical);
        if let Some(clash) = projects.iter().find(|p| p.canonical == home.canonical) {
            return Err(CoreError::new(
                ErrorCode::InvalidScope,
                "the home root cannot also be a project",
            )
            .at(&clash.lexical));
        }
        Ok(NormalizedScope {
            id: ScopeId::for_canonical_home(&home.canonical),
            home,
            projects,
            history_root: raw.history_root.clone(),
            cache_root: raw.cache_root.clone(),
            data_root: raw.data_root.clone(),
            codex_home: raw.codex_home_or_default(),
            raw: raw.clone(),
        })
    }

    /// Lease keys for every physical root, in sorted order.
    ///
    /// Callers acquire leases in this order so two processes never deadlock.
    pub fn lease_keys(&self) -> Vec<LeaseKey> {
        let mut keys: Vec<LeaseKey> = std::iter::once(&self.home)
            .chain(self.projects.iter())
            .map(|root| LeaseKey {
                canonical_root: root.canonical.clone(),
            })
            .collect();
        keys.sort();
        keys.dedup();
        keys
    }

    /// True when `path` lies under the home, a project, or Codex's own
    /// directory ([`Self::codex_home`]), by canonical or lexical prefix.
    /// `codex_home` has no canonical form of its own (see its doc comment),
    /// so it is checked lexically only.
    pub fn contains(&self, path: &Path) -> bool {
        std::iter::once(&self.home)
            .chain(self.projects.iter())
            .any(|root| path.starts_with(&root.canonical) || path.starts_with(&root.lexical))
            || path.starts_with(&self.codex_home)
    }

    /// Rewrites a path for display: `~/...` under the home, absolute
    /// otherwise.
    pub fn display_path(&self, path: &Path) -> String {
        for base in [&self.home.lexical, &self.home.canonical] {
            if let Ok(rest) = path.strip_prefix(base) {
                return format!("~/{}", rest.display());
            }
        }
        path.display().to_string()
    }

    /// The wire form carried by every result envelope.
    pub fn effective(&self) -> EffectiveScope {
        EffectiveScope {
            id: self.id.clone(),
            kind: self.raw.kind,
            home: self.home.lexical.clone(),
            projects: self.projects.iter().map(|p| p.lexical.clone()).collect(),
            history_root: self.history_root.clone(),
        }
    }
}

/// Scope as reported in a result envelope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EffectiveScope {
    /// Stable identity.
    pub id: ScopeId,
    /// Live or fixture.
    pub kind: ScopeKind,
    /// Home as given.
    pub home: PathBuf,
    /// Projects as given, sorted by canonical path.
    pub projects: Vec<PathBuf>,
    /// History directory.
    pub history_root: PathBuf,
}

fn physical(fs: &dyn ScopeFs, lexical: &Path) -> Result<PhysicalRoot, CoreError> {
    let canonical = fs
        .canonicalize(lexical)
        .map_err(|source| CoreError::io(lexical, source).with_code(ErrorCode::InvalidScope))?;
    Ok(PhysicalRoot {
        lexical: lexical.to_path_buf(),
        canonical,
    })
}

impl CoreError {
    fn with_code(mut self, code: ErrorCode) -> Self {
        self.code = code;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureBuilder;

    #[test]
    fn scope_identity_is_stable_under_an_aliased_root() {
        // One physical home, reachable by two names: the real directory and a
        // symlink alias. Projects are also aliased. Nothing touches the disk.
        let fs = FixtureBuilder::new()
            .dir("/vol/homes/alice")
            .dir("/vol/homes/alice/work/app")
            .alias("/Users/alice", "/vol/homes/alice")
            .alias("/Users/alice/work/app-link", "/vol/homes/alice/work/app")
            .build_fs();

        let mut direct = RuntimeScope::fixture("/vol/homes/alice");
        direct.projects = ProjectSelection::Explicit {
            paths: vec![PathBuf::from("/vol/homes/alice/work/app")],
        };
        let mut aliased = RuntimeScope::fixture("/Users/alice");
        aliased.projects = ProjectSelection::Explicit {
            paths: vec![
                PathBuf::from("/Users/alice/work/app-link"),
                PathBuf::from("/vol/homes/alice/work/app"),
            ],
        };

        let a = NormalizedScope::normalize(&direct, &fs).unwrap();
        let b = NormalizedScope::normalize(&aliased, &fs).unwrap();

        assert_eq!(a.id, b.id, "aliases of one home share one scope id");
        assert_eq!(
            a.lease_keys(),
            b.lease_keys(),
            "lease keys follow canonical roots"
        );
        assert_eq!(b.projects.len(), 1, "duplicate project aliases collapse");
        assert_ne!(
            a.home.lexical, b.home.lexical,
            "lexical names are kept for display"
        );
        assert_eq!(
            b.display_path(Path::new("/Users/alice/.agents/skills")),
            "~/.agents/skills"
        );
    }

    #[test]
    fn discovery_fills_projects_and_drops_the_home() {
        use crate::testing::FakeProjectDiscovery;
        let fs = FixtureBuilder::new()
            .dir("/home/u")
            .dir("/home/u/src/app")
            .dir("/home/u/src/skip")
            .build_fs();
        let mut scope = RuntimeScope::live("/home/u", "/home/u/.local/share/skill-studio");
        scope.projects = ProjectSelection::Discover {
            exclude: vec![PathBuf::from("/home/u/src/skip")],
        };

        let err = NormalizedScope::normalize(&scope, &fs).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidScope, "discover needs a port");

        let discovery = FakeProjectDiscovery {
            projects: vec![
                PathBuf::from("/home/u/src/app"),
                PathBuf::from("/home/u/src/skip"),
                PathBuf::from("/home/u/src/gone"),
            ],
        };
        let normalized =
            NormalizedScope::normalize_with_discovery(&scope, &fs, Some(&discovery)).unwrap();
        let found: Vec<_> = normalized.projects.iter().map(|p| &p.lexical).collect();
        assert_eq!(found, [Path::new("/home/u/src/app")]);
        assert_eq!(
            normalized.lease_keys().len(),
            2,
            "discovered roots are leased"
        );

        // Under Discover, a home found by discovery is dropped rather than
        // an error - unlike Explicit, where naming the home fails the scope.
        let discovery = FakeProjectDiscovery {
            projects: vec![PathBuf::from("/home/u")],
        };
        let normalized =
            NormalizedScope::normalize_with_discovery(&scope, &fs, Some(&discovery)).unwrap();
        assert!(normalized.projects.is_empty(), "home is never a project");
    }

    #[test]
    fn normalize_with_discovery_applies_the_saved_project_list() {
        use crate::testing::FakeProjectDiscovery;
        let fs = FixtureBuilder::new()
            .dir("/home/u")
            .dir("/home/u/found")
            .dir("/home/u/added")
            .dir("/home/u/config-excluded")
            .dir("/home/u/request-excluded")
            .file(
                "/home/u/.agents/skill-studio.json",
                br#"{"projects":{"added":["/home/u/added","/home/u"],"excluded":["/home/u/config-excluded"]}}"#,
            )
            .build_fs();
        let mut scope = RuntimeScope::live("/home/u", "/home/u/.local/share/skill-studio");
        scope.projects = ProjectSelection::Discover {
            exclude: vec![PathBuf::from("/home/u/request-excluded")],
        };
        let discovery = FakeProjectDiscovery {
            projects: vec![
                PathBuf::from("/home/u/found"),
                PathBuf::from("/home/u/config-excluded"),
                PathBuf::from("/home/u/request-excluded"),
            ],
        };

        let normalized =
            NormalizedScope::normalize_with_discovery(&scope, &fs, Some(&discovery)).unwrap();
        let found: Vec<_> = normalized.projects.iter().map(|p| &p.lexical).collect();
        assert_eq!(
            found,
            [Path::new("/home/u/added"), Path::new("/home/u/found")],
            "the config's `added` folder appears, its `excluded` folder and the \
             request's own `exclude` are both removed, and home in `added` does not fail"
        );
    }

    #[test]
    fn history_override_needs_fixture_mode() {
        let fs = FixtureBuilder::new().dir("/home/u").build_fs();
        let mut scope = RuntimeScope::live("/home/u", "/home/u/.local/share/skill-studio");
        scope.history_binding = HistoryBinding::Override;
        let err = NormalizedScope::normalize(&scope, &fs).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidScope);
    }
}
