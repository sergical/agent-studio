//! Currency ("update available") per install method - plan.md unit 3.4,
//! user-stories.md U4.
//!
//! Five rules, one per [`crate::identity::SourceKind`]: skills.sh compares
//! the lock file's `skillFolderHash` against the source repo's tree SHA at
//! HEAD, one [`SourceTreeLookup`] call per repo regardless of how many
//! skills within it are checked; dotagents compares the ledger's pinned
//! commit against the newest commit for the skill's path; plugin compares
//! the locally known cache version against the marketplace manifest; a fork
//! compares the home registry's `base_commit` against the newest commit for
//! its recorded upstream, pinned independently of whatever the ledger says
//! for the same name; manual (and anything else - `InRepo`, or a deployment
//! with no ledger owner) is never a candidate. Ported from the desktop's
//! `skill_update_check.rs`, which this replaces for the skills.sh
//! comparison: that file used to shell one `gh api` commits lookup per
//! skill; this reads the tree once per source repo instead.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::dotagents_ledger::{self, DotagentsSkill};
use crate::error::CoreError;
use crate::identity::SourceKind;
use crate::lock_file::{self, SkillLockFile};
use crate::ports::ScopeFs;

/// One skill's currency, keyed by name in [`outdated`]'s result. Wire-visible:
/// `ops::outdated`'s CLI subcommand and MCP tool return this map directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Currency {
    /// The installed side matches the newest side.
    UpToDate,
    /// The installed side is behind the newest side.
    UpdateAvailable,
    /// No install method claims this skill: manual, in-repo, an ambiguous
    /// owner, or a fork with no registry row (or no recorded upstream).
    /// Never shown as "update available".
    NotTracked,
    /// A method claims the skill but the check could not run: a missing
    /// lock/ledger entry, a malformed lock file, or a lookup failure.
    /// Distinct from `NotTracked` so the UI can tell "we don't know" from
    /// "there is nothing to know" - and, per the crash test below, a
    /// malformed lock file must resolve every skills.sh skill to `Unknown`,
    /// never silently to `UpToDate` or `UpdateAvailable`.
    Unknown,
}

/// A commit's identity and when it landed, returned by [`CommitLookup`].
/// `committed_at` is `None` for a skills.sh tree SHA, which carries no date
/// of its own (see [`OutdatedRecord::latest_commit_at`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CommitInfo {
    /// The commit's SHA, or a skills.sh tree SHA for that folder.
    pub sha: String,
    /// The commit's committer date, ISO 8601. `None` for a skills.sh tree
    /// SHA (GitHub's tree listing carries no date).
    pub committed_at: Option<String>,
}

/// One skill's full currency result, keyed by name in [`outdated`]'s result.
/// Wire-visible: `ops::outdated`'s CLI subcommand and MCP tool return this
/// map directly. Carries the compared pair alongside [`Currency`] so a
/// caller can show what changed, not just that it did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutdatedRecord {
    /// The currency rule's verdict.
    pub currency: Currency,
    /// The installed side: a dotagents pinned commit SHA, or a skills.sh
    /// `skillFolderHash`. `None` when the currency rule never resolved an
    /// installed side (`NotTracked`, or `Unknown` before the lock/ledger
    /// entry was found).
    pub installed_commit: Option<String>,
    /// The newest side: a dotagents commit SHA, or a skills.sh tree SHA for
    /// that folder. `None` on the same conditions as `installed_commit`, or
    /// when the lookup failed.
    pub latest_commit: Option<String>,
    /// The newest commit's date, dotagents only - `None` for skills.sh
    /// (tree SHAs carry no date) and whenever `latest_commit` is `None`.
    pub latest_commit_at: Option<String>,
    /// Why the lookup could not confirm currency, when `currency` is
    /// [`Currency::Unknown`] because of a lookup failure rather than a
    /// missing lock/ledger entry. `None` otherwise.
    pub error: Option<String>,
}

impl OutdatedRecord {
    /// A record with no installed/latest side and no error - `NotTracked`,
    /// or the "no entry found" shade of `Unknown`.
    fn bare(currency: Currency) -> Self {
        Self {
            currency,
            installed_commit: None,
            latest_commit: None,
            latest_commit_at: None,
            error: None,
        }
    }

    /// `Unknown`, naming why the lookup itself failed.
    fn unknown_with_error(installed_commit: Option<String>, error: String) -> Self {
        Self {
            currency: Currency::Unknown,
            installed_commit,
            latest_commit: None,
            latest_commit_at: None,
            error: Some(error),
        }
    }
}

/// One skill's install-method classification, derived by the caller from a
/// scanned [`crate::dto::Inventory`] (`ops::outdated` does this). A separate
/// input type, rather than [`crate::dto::DeploymentDto`] itself, so this
/// module's tests don't need to build one of those by hand.
pub struct OutdatedTarget {
    /// Skill name - the lock file and dotagents ledger key, and the key
    /// `outdated` returns results under.
    pub name: String,
    /// Which currency rule applies.
    pub source_kind: SourceKind,
    /// `(marketplace, plugin, installed_version)` from the deployment's
    /// [`crate::dto::PluginSourceDto`]. `Some` only when `source_kind` is
    /// [`SourceKind::Plugin`]; `None` there too when the plugin cache path
    /// carried no version directory.
    pub plugin: Option<(String, String, Option<String>)>,
}

/// Looks up every subtree's SHA in a GitHub repo at HEAD, in one call -
/// GitHub's recursive tree listing returns every subdirectory's SHA at once,
/// so N skills sharing one source repo cost one network call, not N. Keyed
/// by path relative to the repo root, matching `skillFolderHash`'s scope.
pub trait SourceTreeLookup: Send + Sync {
    /// `Ok(shas)` on success, keyed by path; a repo that cannot be reached
    /// at all is `Err`, not an empty map (an empty map would read as "no
    /// skill here" for every path, which is not the same failure).
    fn tree_shas_at_head(&self, repo: &str) -> Result<HashMap<String, String>, CoreError>;
}

/// Looks up the newest commit that touched `path` in `repo` - the dotagents
/// currency check's one network call per dotagents skill.
pub trait CommitLookup: Send + Sync {
    /// `Ok(None)` when `path` has no commits (yet), not an error.
    fn latest_commit(&self, repo: &str, path: &str) -> Result<Option<CommitInfo>, CoreError>;
}

/// Looks up a plugin's current version from its marketplace manifest -
/// Claude Code only, since Codex has no plugin CLI to publish one.
pub trait PluginManifestLookup: Send + Sync {
    /// `Ok(None)` when the marketplace has no version recorded for the
    /// plugin (not an error - it just can't confirm currency).
    fn marketplace_version(
        &self,
        marketplace: &str,
        plugin: &str,
    ) -> Result<Option<String>, CoreError>;
}

/// Currency for every target in `targets`, keyed by name. Reads the shared
/// skills.sh lock file and dotagents ledger under `home/.agents` through
/// `fs`; a malformed or oversized lock file (see [`lock_file::read_lock_file`])
/// resolves every skills.sh target to [`Currency::Unknown`] rather than
/// failing the whole check or reporting a false "update available".
pub fn outdated(
    fs: &dyn ScopeFs,
    home: &Path,
    targets: &[OutdatedTarget],
    tree_lookup: &dyn SourceTreeLookup,
    commit_lookup: &dyn CommitLookup,
    plugin_lookup: &dyn PluginManifestLookup,
) -> BTreeMap<String, OutdatedRecord> {
    let agents_dir = home.join(".agents");
    let lock = lock_file::read_lock_file(fs, &lock_file::lock_file_path(home));
    let dotagents = dotagents_ledger::read_dotagents_ledger(fs, &agents_dir).unwrap_or_default();
    let home_registry = crate::ownership::read_home_registry_result(fs, home);

    // One `tree_shas_at_head` call per distinct repo, however many
    // skills.sh targets share it - the performance number this unit
    // measures.
    let mut tree_cache: HashMap<String, Result<HashMap<String, String>, CoreError>> =
        HashMap::new();

    let mut results = BTreeMap::new();
    for target in targets {
        let record = match target.source_kind {
            SourceKind::SkillsSh => {
                skills_sh_currency(&target.name, &lock, tree_lookup, &mut tree_cache)
            }
            SourceKind::Dotagents => dotagents_currency(&target.name, &dotagents, commit_lookup),
            SourceKind::Plugin => plugin_currency(target, plugin_lookup),
            SourceKind::Fork => fork_currency(&target.name, &home_registry, commit_lookup),
            SourceKind::InRepo | SourceKind::Manual => OutdatedRecord::bare(Currency::NotTracked),
        };
        results.insert(target.name.clone(), record);
    }
    results
}

/// Normalizes a repo slug so two spellings of the same source (a
/// `github.com/` prefix, a trailing `.git`, or a different case) share one
/// `tree_cache` entry and one [`SourceTreeLookup`] call, instead of one each.
/// Public so the desktop's own tree cache (`skill_update_check::tree_shas_cached`)
/// can key off the same normalization until the F2 rewire lands (unit 3.4
/// follow-up).
pub fn normalize_repo_key(repo: &str) -> String {
    let lower = repo.to_ascii_lowercase();
    let stripped = lower.strip_prefix("github.com/").unwrap_or(lower.as_str());
    stripped
        .strip_suffix(".git")
        .unwrap_or(stripped)
        .to_string()
}

fn skills_sh_currency(
    name: &str,
    lock: &Result<SkillLockFile, CoreError>,
    tree_lookup: &dyn SourceTreeLookup,
    tree_cache: &mut HashMap<String, Result<HashMap<String, String>, CoreError>>,
) -> OutdatedRecord {
    let Ok(lock) = lock else {
        return OutdatedRecord::bare(Currency::Unknown);
    };
    let Some(entry) = lock.skills.get(name) else {
        return OutdatedRecord::bare(Currency::Unknown);
    };
    let installed_commit = Some(entry.skill_folder_hash.clone());
    let Some(repo) = dotagents_ledger::github_repo_from_source(&entry.source) else {
        return OutdatedRecord::bare(Currency::Unknown);
    };
    let repo = normalize_repo_key(&repo);
    let Some(skill_path) = entry.skill_path.as_deref() else {
        return OutdatedRecord::bare(Currency::Unknown);
    };
    let folder_path = skill_path.strip_suffix("/SKILL.md").unwrap_or(skill_path);

    let tree = tree_cache
        .entry(repo.clone())
        .or_insert_with(|| tree_lookup.tree_shas_at_head(&repo));
    match tree {
        Ok(shas) => match shas.get(folder_path) {
            Some(sha) if sha == &entry.skill_folder_hash => OutdatedRecord {
                currency: Currency::UpToDate,
                installed_commit,
                latest_commit: Some(sha.clone()),
                latest_commit_at: None,
                error: None,
            },
            Some(sha) => OutdatedRecord {
                currency: Currency::UpdateAvailable,
                installed_commit,
                latest_commit: Some(sha.clone()),
                latest_commit_at: None,
                error: None,
            },
            None => OutdatedRecord {
                currency: Currency::Unknown,
                installed_commit,
                latest_commit: None,
                latest_commit_at: None,
                error: Some(format!("{folder_path} not found in {repo}'s source tree")),
            },
        },
        Err(e) => OutdatedRecord::unknown_with_error(installed_commit, e.message.clone()),
    }
}

fn dotagents_currency(
    name: &str,
    ledger: &[DotagentsSkill],
    commit_lookup: &dyn CommitLookup,
) -> OutdatedRecord {
    let Some(entry) = ledger.iter().find(|skill| skill.name == name) else {
        return OutdatedRecord::bare(Currency::Unknown);
    };
    let (Some(repo), Some(installed)) = (&entry.github_repo, &entry.installed_commit) else {
        return OutdatedRecord::bare(Currency::Unknown);
    };
    let installed_commit = Some(installed.clone());
    match commit_lookup.latest_commit(repo, &entry.path) {
        Ok(Some(latest)) if &latest.sha == installed => OutdatedRecord {
            currency: Currency::UpToDate,
            installed_commit,
            latest_commit: Some(latest.sha),
            latest_commit_at: latest.committed_at,
            error: None,
        },
        Ok(Some(latest)) => OutdatedRecord {
            currency: Currency::UpdateAvailable,
            installed_commit,
            latest_commit: Some(latest.sha),
            latest_commit_at: latest.committed_at,
            error: None,
        },
        Ok(None) => OutdatedRecord {
            currency: Currency::Unknown,
            installed_commit,
            latest_commit: None,
            latest_commit_at: None,
            error: None,
        },
        Err(e) => OutdatedRecord::unknown_with_error(installed_commit, e.message.clone()),
    }
}

/// A forked skill's currency: pinned to its `base_commit` rather than
/// whatever the dotagents/skills.sh ledger says for the same name -
/// forking detaches a skill from its ledger, exactly as
/// `classify_owner`'s `LifecycleOwnerKind::Fork` precedence already treats
/// it for ownership. `NotTracked` for a fork with no registry row, or one
/// with no recorded upstream (a legacy record); `Unknown` plus `error` when
/// the registry itself could not be read.
fn fork_currency(
    name: &str,
    home_registry: &Result<crate::ownership::HomeRegistry, CoreError>,
    commit_lookup: &dyn CommitLookup,
) -> OutdatedRecord {
    let home_registry = match home_registry {
        Ok(registry) => registry,
        Err(e) => return OutdatedRecord::unknown_with_error(None, e.message.clone()),
    };
    let Some(record) = home_registry.forks.get(name) else {
        return OutdatedRecord::bare(Currency::NotTracked);
    };
    if record.repo.is_empty() || record.path.is_empty() || record.base_commit.is_empty() {
        return OutdatedRecord::bare(Currency::NotTracked);
    }
    let installed_commit = Some(record.base_commit.clone());
    match commit_lookup.latest_commit(&record.repo, &record.path) {
        Ok(Some(latest)) if latest.sha == record.base_commit => OutdatedRecord {
            currency: Currency::UpToDate,
            installed_commit,
            latest_commit: Some(latest.sha),
            latest_commit_at: latest.committed_at,
            error: None,
        },
        Ok(Some(latest)) => OutdatedRecord {
            currency: Currency::UpdateAvailable,
            installed_commit,
            latest_commit: Some(latest.sha),
            latest_commit_at: latest.committed_at,
            error: None,
        },
        Ok(None) => OutdatedRecord {
            currency: Currency::Unknown,
            installed_commit,
            latest_commit: None,
            latest_commit_at: None,
            error: None,
        },
        Err(e) => OutdatedRecord::unknown_with_error(installed_commit, e.message.clone()),
    }
}

fn plugin_currency(
    target: &OutdatedTarget,
    plugin_lookup: &dyn PluginManifestLookup,
) -> OutdatedRecord {
    let Some((marketplace, plugin, installed_version)) = &target.plugin else {
        return OutdatedRecord::bare(Currency::Unknown);
    };
    let Some(installed) = installed_version else {
        return OutdatedRecord::bare(Currency::Unknown);
    };
    let installed_commit = Some(installed.clone());
    match plugin_lookup.marketplace_version(marketplace, plugin) {
        Ok(Some(latest)) if &latest == installed => OutdatedRecord {
            currency: Currency::UpToDate,
            installed_commit,
            latest_commit: Some(latest),
            latest_commit_at: None,
            error: None,
        },
        Ok(Some(latest)) => OutdatedRecord {
            currency: Currency::UpdateAvailable,
            installed_commit,
            latest_commit: Some(latest),
            latest_commit_at: None,
            error: None,
        },
        Ok(None) => OutdatedRecord {
            currency: Currency::Unknown,
            installed_commit,
            latest_commit: None,
            latest_commit_at: None,
            error: None,
        },
        Err(e) => OutdatedRecord::unknown_with_error(installed_commit, e.message.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::FixtureBuilder;
    use std::sync::Mutex;

    /// Scripted `SourceTreeLookup`: one answer per repo, and a call counter
    /// so tests can assert "one call per repo" directly.
    #[derive(Default)]
    struct FakeTreeLookup {
        answers: HashMap<String, HashMap<String, String>>,
        calls: Mutex<Vec<String>>,
    }

    impl SourceTreeLookup for FakeTreeLookup {
        fn tree_shas_at_head(&self, repo: &str) -> Result<HashMap<String, String>, CoreError> {
            self.calls.lock().unwrap().push(repo.to_string());
            Ok(self.answers.get(repo).cloned().unwrap_or_default())
        }
    }

    struct NoCommits;
    impl CommitLookup for NoCommits {
        fn latest_commit(&self, _repo: &str, _path: &str) -> Result<Option<CommitInfo>, CoreError> {
            Ok(None)
        }
    }

    struct NoPlugins;
    impl PluginManifestLookup for NoPlugins {
        fn marketplace_version(
            &self,
            _marketplace: &str,
            _plugin: &str,
        ) -> Result<Option<String>, CoreError> {
            Ok(None)
        }
    }

    fn write_skill_lock(fs_builder: FixtureBuilder, name: &str, hash: &str) -> FixtureBuilder {
        let json = serde_json::json!({
            "version": 3,
            "skills": {
                name: {
                    "source": "obra/write-tests",
                    "sourceType": "github",
                    "sourceUrl": "https://github.com/obra/write-tests",
                    "skillPath": format!("skills/{name}/SKILL.md"),
                    "skillFolderHash": hash,
                    "installedAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                }
            }
        });
        fs_builder.file(
            "/home/.agents/.skill-lock.json",
            &serde_json::to_string(&json).unwrap().into_bytes(),
        )
    }

    /// Flow: a skills.sh skill whose lock `skillFolderHash` differs from the
    /// source tree's SHA for its path.
    /// Expectation: `Currency::UpdateAvailable`.
    /// A failure here means the hash comparison was dropped in favor of
    /// always trusting the lock file, or names the wrong skill's row.
    #[test]
    fn skills_sh_stale_hash_shows_update_available_or_names_the_missing_row() {
        let fs = write_skill_lock(
            FixtureBuilder::new().dir("/home/.agents"),
            "write-tests",
            "old-hash",
        )
        .build_fs();
        let mut answers = HashMap::new();
        answers.insert(
            "obra/write-tests".to_string(),
            HashMap::from([("skills/write-tests".to_string(), "new-hash".to_string())]),
        );
        let tree_lookup = FakeTreeLookup {
            answers,
            calls: Mutex::new(Vec::new()),
        };
        let targets = vec![OutdatedTarget {
            name: "write-tests".to_string(),
            source_kind: SourceKind::SkillsSh,
            plugin: None,
        }];
        let result = outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &tree_lookup,
            &NoCommits,
            &NoPlugins,
        );
        assert_eq!(result["write-tests"].currency, Currency::UpdateAvailable);
        assert_eq!(
            result["write-tests"].installed_commit.as_deref(),
            Some("old-hash")
        );
        assert_eq!(
            result["write-tests"].latest_commit.as_deref(),
            Some("new-hash")
        );
    }

    /// Flow: a skills.sh skill whose lock hash matches the source tree's SHA.
    /// Expectation: `Currency::UpToDate`, not `UpdateAvailable`.
    /// A failure here means the comparison always reports a mismatch (a
    /// false positive on every check), or names the mismatch it found.
    #[test]
    fn skills_sh_current_hash_shows_nothing_or_names_the_false_positive() {
        let fs = write_skill_lock(
            FixtureBuilder::new().dir("/home/.agents"),
            "write-tests",
            "same-hash",
        )
        .build_fs();
        let mut answers = HashMap::new();
        answers.insert(
            "obra/write-tests".to_string(),
            HashMap::from([("skills/write-tests".to_string(), "same-hash".to_string())]),
        );
        let tree_lookup = FakeTreeLookup {
            answers,
            calls: Mutex::new(Vec::new()),
        };
        let targets = vec![OutdatedTarget {
            name: "write-tests".to_string(),
            source_kind: SourceKind::SkillsSh,
            plugin: None,
        }];
        let result = outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &tree_lookup,
            &NoCommits,
            &NoPlugins,
        );
        assert_eq!(result["write-tests"].currency, Currency::UpToDate);
    }

    /// Flow: a manual skill, with no owner record in any ledger.
    /// Expectation: `Currency::NotTracked`, never `UpToDate`.
    /// A failure here means a manual skill fell through to a default that
    /// reads as "current" or "outdated" instead of "not tracked".
    #[test]
    fn manual_skill_shows_not_tracked_and_never_up_to_date() {
        let fs = FixtureBuilder::new().dir("/home/.agents").build_fs();
        let targets = vec![OutdatedTarget {
            name: "hand-placed".to_string(),
            source_kind: SourceKind::Manual,
            plugin: None,
        }];
        let result = outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &FakeTreeLookup::default(),
            &NoCommits,
            &NoPlugins,
        );
        assert_eq!(result["hand-placed"].currency, Currency::NotTracked);
        assert_ne!(result["hand-placed"].currency, Currency::UpToDate);
    }

    /// Flow: three skills.sh skills installed from the same source repo.
    /// Expectation: exactly one `tree_shas_at_head` call for that repo.
    /// A failure here means the batching cache was dropped and the check
    /// went back to one network call per skill.
    #[test]
    fn the_check_makes_exactly_one_network_call_per_source_repo_not_per_skill() {
        let mut fs_builder = FixtureBuilder::new().dir("/home/.agents");
        let mut skills = serde_json::Map::new();
        for name in ["a", "b", "c"] {
            skills.insert(
                name.to_string(),
                serde_json::json!({
                    "source": "obra/write-tests",
                    "sourceType": "github",
                    "sourceUrl": "https://github.com/obra/write-tests",
                    "skillPath": format!("skills/{name}/SKILL.md"),
                    "skillFolderHash": "hash",
                    "installedAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                }),
            );
        }
        let json = serde_json::json!({ "version": 3, "skills": skills });
        fs_builder = fs_builder.file(
            "/home/.agents/.skill-lock.json",
            &serde_json::to_string(&json).unwrap().into_bytes(),
        );
        let fs = fs_builder.build_fs();

        let tree_lookup = FakeTreeLookup::default();
        let targets: Vec<OutdatedTarget> = ["a", "b", "c"]
            .iter()
            .map(|name| OutdatedTarget {
                name: (*name).to_string(),
                source_kind: SourceKind::SkillsSh,
                plugin: None,
            })
            .collect();
        outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &tree_lookup,
            &NoCommits,
            &NoPlugins,
        );
        assert_eq!(*tree_lookup.calls.lock().unwrap(), vec!["obra/write-tests"]);
    }

    /// Flow: two skills.sh skills whose lock entries name the same GitHub
    /// repo with different spellings (`owner/repo` and
    /// `https://github.com/Owner/Repo.git`, via `git:` sources).
    /// Expectation: exactly one `tree_shas_at_head` call, for the normalized
    /// key.
    /// A failure here means the two spellings landed in different
    /// `tree_cache` entries and the check cost a second network call, or
    /// names the extra repo key it called.
    #[test]
    fn two_spellings_of_one_repo_cost_one_tree_call_or_names_the_second_call() {
        let json = serde_json::json!({
            "version": 3,
            "skills": {
                "a": {
                    "source": "obra/write-tests",
                    "sourceType": "github",
                    "sourceUrl": "https://github.com/obra/write-tests",
                    "skillPath": "skills/a/SKILL.md",
                    "skillFolderHash": "hash",
                    "installedAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                },
                "b": {
                    "source": "git:https://github.com/Obra/Write-Tests.git",
                    "sourceType": "github",
                    "sourceUrl": "https://github.com/Obra/Write-Tests",
                    "skillPath": "skills/b/SKILL.md",
                    "skillFolderHash": "hash",
                    "installedAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                }
            }
        });
        let fs = FixtureBuilder::new()
            .dir("/home/.agents")
            .file(
                "/home/.agents/.skill-lock.json",
                &serde_json::to_string(&json).unwrap().into_bytes(),
            )
            .build_fs();

        let tree_lookup = FakeTreeLookup::default();
        let targets = vec![
            OutdatedTarget {
                name: "a".to_string(),
                source_kind: SourceKind::SkillsSh,
                plugin: None,
            },
            OutdatedTarget {
                name: "b".to_string(),
                source_kind: SourceKind::SkillsSh,
                plugin: None,
            },
        ];
        outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &tree_lookup,
            &NoCommits,
            &NoPlugins,
        );
        assert_eq!(*tree_lookup.calls.lock().unwrap(), vec!["obra/write-tests"]);
    }

    /// Flow: a plugin skill whose locally known cache version differs from
    /// the marketplace manifest's version.
    /// Expectation: `Currency::UpdateAvailable`.
    /// A failure here means the plugin arm was never wired up (plugin skills
    /// stayed uncheckable, matching today's gap) or compared the wrong pair
    /// of versions.
    #[test]
    fn plugin_skill_compares_cache_version_to_marketplace_or_names_the_missing_comparison() {
        struct FakePluginLookup;
        impl PluginManifestLookup for FakePluginLookup {
            fn marketplace_version(
                &self,
                marketplace: &str,
                plugin: &str,
            ) -> Result<Option<String>, CoreError> {
                assert_eq!(marketplace, "anthropic-plugins");
                assert_eq!(plugin, "openai-templates");
                Ok(Some("2.0.0".to_string()))
            }
        }

        let fs = FixtureBuilder::new().dir("/home/.agents").build_fs();
        let targets = vec![OutdatedTarget {
            name: "openai-templates".to_string(),
            source_kind: SourceKind::Plugin,
            plugin: Some((
                "anthropic-plugins".to_string(),
                "openai-templates".to_string(),
                Some("1.0.0".to_string()),
            )),
        }];
        let result = outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &FakeTreeLookup::default(),
            &NoCommits,
            &FakePluginLookup,
        );
        assert_eq!(
            result["openai-templates"].currency,
            Currency::UpdateAvailable
        );
    }

    /// Flow: a lock file that fails to parse (the crash/failure case this
    /// unit names).
    /// Expectation: every skills.sh target resolves to `Currency::Unknown`,
    /// and the call does not panic or mark the skill `UpdateAvailable`.
    /// A failure here means a malformed lock file crashes the check, or a
    /// parse error is silently treated as "no entry, so nothing installed
    /// is outdated" - both wrong, since the real state is unknown, not
    /// current.
    #[test]
    fn malformed_lock_file_yields_unknown_for_skills_sh_or_names_the_crash() {
        let fs = FixtureBuilder::new()
            .dir("/home/.agents")
            .file("/home/.agents/.skill-lock.json", b"not json")
            .build_fs();
        let targets = vec![OutdatedTarget {
            name: "write-tests".to_string(),
            source_kind: SourceKind::SkillsSh,
            plugin: None,
        }];
        let result = outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &FakeTreeLookup::default(),
            &NoCommits,
            &NoPlugins,
        );
        assert_eq!(result["write-tests"].currency, Currency::Unknown);
    }

    /// Flow: a dotagents skill whose `CommitLookup` returns a newer commit
    /// and a committer date.
    /// Expectation: the record carries both the installed and the latest
    /// SHA, plus the latest commit's date.
    /// A failure here means the record dropped the SHA/date pair the desktop
    /// used to compute from its own `CommitLookup`, forcing a caller back to
    /// a second network round trip just to show "as of <date>".
    #[test]
    fn dotagents_newer_commit_carries_installed_and_latest_sha_and_date() {
        struct DatedCommit;
        impl CommitLookup for DatedCommit {
            fn latest_commit(
                &self,
                _repo: &str,
                _path: &str,
            ) -> Result<Option<CommitInfo>, CoreError> {
                Ok(Some(CommitInfo {
                    sha: "new-sha".to_string(),
                    committed_at: Some("2026-02-01T00:00:00Z".to_string()),
                }))
            }
        }

        let fs = FixtureBuilder::new()
            .dir("/home/.agents")
            .file(
                "/home/.agents/agents.lock",
                br#"
[skills.find-bugs]
source = "getsentry/find-bugs"
resolved_path = "skills/find-bugs"
resolved_commit = "old-sha"
"#,
            )
            .build_fs();
        let targets = vec![OutdatedTarget {
            name: "find-bugs".to_string(),
            source_kind: SourceKind::Dotagents,
            plugin: None,
        }];
        let result = outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &FakeTreeLookup::default(),
            &DatedCommit,
            &NoPlugins,
        );
        let record = &result["find-bugs"];
        assert_eq!(record.currency, Currency::UpdateAvailable);
        assert_eq!(record.installed_commit.as_deref(), Some("old-sha"));
        assert_eq!(record.latest_commit.as_deref(), Some("new-sha"));
        assert_eq!(
            record.latest_commit_at.as_deref(),
            Some("2026-02-01T00:00:00Z")
        );
    }

    /// Flow: a skills.sh skill whose lock hash matches the source tree's SHA.
    /// Expectation: the record's `installed_commit` and `latest_commit` both
    /// carry the shared hash, with no `latest_commit_at` - a tree SHA has no
    /// date of its own.
    /// A failure here means the record dropped the skills.sh hash/tree-SHA
    /// pair, or invented a date a tree lookup never returned.
    #[test]
    fn skills_sh_record_carries_hash_and_tree_sha_with_no_date() {
        let fs = write_skill_lock(
            FixtureBuilder::new().dir("/home/.agents"),
            "write-tests",
            "same-hash",
        )
        .build_fs();
        let mut answers = HashMap::new();
        answers.insert(
            "obra/write-tests".to_string(),
            HashMap::from([("skills/write-tests".to_string(), "same-hash".to_string())]),
        );
        let tree_lookup = FakeTreeLookup {
            answers,
            calls: Mutex::new(Vec::new()),
        };
        let targets = vec![OutdatedTarget {
            name: "write-tests".to_string(),
            source_kind: SourceKind::SkillsSh,
            plugin: None,
        }];
        let result = outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &tree_lookup,
            &NoCommits,
            &NoPlugins,
        );
        let record = &result["write-tests"];
        assert_eq!(record.installed_commit.as_deref(), Some("same-hash"));
        assert_eq!(record.latest_commit.as_deref(), Some("same-hash"));
        assert_eq!(record.latest_commit_at, None);
    }

    /// Flow: a dotagents `CommitLookup` fails outright (network error).
    /// Expectation: `Currency::Unknown`, and the record's `error` names the
    /// failure - not silently dropped.
    /// A failure here means a lookup error was swallowed into a bare
    /// `Unknown` with no message, leaving the caller unable to show why the
    /// check could not confirm currency.
    #[test]
    fn a_lookup_error_gives_unknown_with_the_message_or_names_the_swallowed_error() {
        struct FailingCommits;
        impl CommitLookup for FailingCommits {
            fn latest_commit(
                &self,
                _repo: &str,
                _path: &str,
            ) -> Result<Option<CommitInfo>, CoreError> {
                Err(CoreError::new(
                    crate::error::ErrorCode::Unsupported,
                    "network unreachable",
                ))
            }
        }

        let fs = FixtureBuilder::new()
            .dir("/home/.agents")
            .file(
                "/home/.agents/agents.lock",
                br#"
[skills.find-bugs]
source = "getsentry/find-bugs"
resolved_path = "skills/find-bugs"
resolved_commit = "old-sha"
"#,
            )
            .build_fs();
        let targets = vec![OutdatedTarget {
            name: "find-bugs".to_string(),
            source_kind: SourceKind::Dotagents,
            plugin: None,
        }];
        let result = outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &FakeTreeLookup::default(),
            &FailingCommits,
            &NoPlugins,
        );
        let record = &result["find-bugs"];
        assert_eq!(record.currency, Currency::Unknown);
        assert_eq!(record.error.as_deref(), Some("network unreachable"));
    }

    /// Flow: a forked skill (ported from the desktop's
    /// `forked_skill_is_a_candidate_pinned_to_its_base_commit_and_wins_over_the_ledger`).
    /// Expectation: the record is pinned to the registry's `base_commit`,
    /// not the ledger's `resolved_commit` for the same name - a fork wins
    /// over the ledger, matching `classify_owner`'s `Fork` precedence.
    /// A failure here means `SourceKind::Fork` fell back to `NotTracked`
    /// (the arm this test's edit replaced) or read the ledger's commit
    /// instead of the registry's.
    #[test]
    fn forked_skill_is_pinned_to_its_base_commit_and_wins_over_the_ledger() {
        struct DatedCommit;
        impl CommitLookup for DatedCommit {
            fn latest_commit(
                &self,
                _repo: &str,
                _path: &str,
            ) -> Result<Option<CommitInfo>, CoreError> {
                Ok(Some(CommitInfo {
                    sha: "new-sha".to_string(),
                    committed_at: Some("2026-02-01T00:00:00Z".to_string()),
                }))
            }
        }

        let fs = FixtureBuilder::new()
            .dir("/home/.agents")
            .file(
                "/home/.agents/agents.lock",
                br#"
[skills.find-bugs]
source = "getsentry/find-bugs"
resolved_path = "skills/find-bugs"
resolved_commit = "ledger-sha"
"#,
            )
            .file(
                "/home/.agents/skill-studio.json",
                br#"{
                    "forks": {
                        "find-bugs": {
                            "deployment_id": "",
                            "skill_dir": "",
                            "repo": "getsentry/find-bugs",
                            "path": "skills/find-bugs",
                            "base_commit": "base-sha"
                        }
                    }
                }"#,
            )
            .build_fs();
        let targets = vec![OutdatedTarget {
            name: "find-bugs".to_string(),
            source_kind: SourceKind::Fork,
            plugin: None,
        }];
        let result = outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &FakeTreeLookup::default(),
            &DatedCommit,
            &NoPlugins,
        );
        let record = &result["find-bugs"];
        assert_eq!(record.currency, Currency::UpdateAvailable);
        assert_eq!(record.installed_commit.as_deref(), Some("base-sha"));
        assert_eq!(record.latest_commit.as_deref(), Some("new-sha"));
    }

    /// Flow: a skill classified `SourceKind::Fork` with no matching row in
    /// `skill-studio.json`'s `forks` bucket at all (e.g. the registry write
    /// raced with the scan that classified it).
    /// Expectation: `Currency::NotTracked`, the same as a fork with no
    /// upstream recorded - nothing to compare, so no error either.
    /// A failure here means a missing row was read as `Unknown` (as if the
    /// registry itself failed to read) instead of "nothing to check".
    #[test]
    fn fork_without_a_registry_row_is_not_tracked() {
        let fs = FixtureBuilder::new().dir("/home/.agents").build_fs();
        let targets = vec![OutdatedTarget {
            name: "find-bugs".to_string(),
            source_kind: SourceKind::Fork,
            plugin: None,
        }];
        let result = outdated(
            &fs,
            Path::new("/home"),
            &targets,
            &FakeTreeLookup::default(),
            &NoCommits,
            &NoPlugins,
        );
        let record = &result["find-bugs"];
        assert_eq!(record.currency, Currency::NotTracked);
        assert_eq!(record.error, None);
    }
}
