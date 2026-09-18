//! Currency ("update available") per install method - plan.md unit 3.4,
//! user-stories.md U4.
//!
//! Four rules, one per [`crate::identity::SourceKind`]: skills.sh compares
//! the lock file's `skillFolderHash` against the source repo's tree SHA at
//! HEAD, one [`SourceTreeLookup`] call per repo regardless of how many
//! skills within it are checked; dotagents compares the ledger's pinned
//! commit against the newest commit for the skill's path; plugin compares
//! the locally known cache version against the marketplace manifest;
//! manual (and anything else - `InRepo`, `Fork`, or a deployment with no
//! ledger owner) is never a candidate. Ported from the desktop's
//! `skill_update_check.rs`, which this replaces for the skills.sh
//! comparison: that file used to shell one `gh api` commits lookup per
//! skill; this reads the tree once per source repo instead.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use crate::dotagents_ledger::{self, DotagentsSkill};
use crate::error::CoreError;
use crate::identity::SourceKind;
use crate::lock_file::{self, SkillLockFile};
use crate::ports::ScopeFs;

/// One skill's currency, keyed by name in [`outdated`]'s result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Currency {
    /// The installed side matches the newest side.
    UpToDate,
    /// The installed side is behind the newest side.
    UpdateAvailable,
    /// No install method claims this skill: manual, in-repo, a fork, or an
    /// ambiguous owner. Never shown as "update available".
    NotTracked,
    /// A method claims the skill but the check could not run: a missing
    /// lock/ledger entry, a malformed lock file, or a lookup failure.
    /// Distinct from `NotTracked` so the UI can tell "we don't know" from
    /// "there is nothing to know" - and, per the crash test below, a
    /// malformed lock file must resolve every skills.sh skill to `Unknown`,
    /// never silently to `UpToDate` or `UpdateAvailable`.
    Unknown,
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
    fn latest_commit(&self, repo: &str, path: &str) -> Result<Option<String>, CoreError>;
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
) -> BTreeMap<String, Currency> {
    let agents_dir = home.join(".agents");
    let lock = lock_file::read_lock_file(fs, &lock_file::lock_file_path(home));
    let dotagents = dotagents_ledger::read_dotagents_ledger(fs, &agents_dir).unwrap_or_default();

    // One `tree_shas_at_head` call per distinct repo, however many
    // skills.sh targets share it - the performance number this unit
    // measures.
    let mut tree_cache: HashMap<String, Result<HashMap<String, String>, CoreError>> =
        HashMap::new();

    let mut results = BTreeMap::new();
    for target in targets {
        let currency = match target.source_kind {
            SourceKind::SkillsSh => {
                skills_sh_currency(&target.name, &lock, tree_lookup, &mut tree_cache)
            }
            SourceKind::Dotagents => dotagents_currency(&target.name, &dotagents, commit_lookup),
            SourceKind::Plugin => plugin_currency(target, plugin_lookup),
            SourceKind::InRepo | SourceKind::Manual | SourceKind::Fork => Currency::NotTracked,
        };
        results.insert(target.name.clone(), currency);
    }
    results
}

/// Normalizes a repo slug so two spellings of the same source (a
/// `github.com/` prefix, a trailing `.git`, or a different case) share one
/// `tree_cache` entry and one [`SourceTreeLookup`] call, instead of one each.
fn normalize_repo_key(repo: &str) -> String {
    let lower = repo.to_ascii_lowercase();
    let stripped = lower
        .strip_prefix("github.com/")
        .unwrap_or(lower.as_str());
    stripped.strip_suffix(".git").unwrap_or(stripped).to_string()
}

fn skills_sh_currency(
    name: &str,
    lock: &Result<SkillLockFile, CoreError>,
    tree_lookup: &dyn SourceTreeLookup,
    tree_cache: &mut HashMap<String, Result<HashMap<String, String>, CoreError>>,
) -> Currency {
    let Ok(lock) = lock else {
        return Currency::Unknown;
    };
    let Some(entry) = lock.skills.get(name) else {
        return Currency::Unknown;
    };
    let Some(repo) = dotagents_ledger::github_repo_from_source(&entry.source) else {
        return Currency::Unknown;
    };
    let repo = normalize_repo_key(&repo);
    let Some(skill_path) = entry.skill_path.as_deref() else {
        return Currency::Unknown;
    };
    let folder_path = skill_path.strip_suffix("/SKILL.md").unwrap_or(skill_path);

    let tree = tree_cache
        .entry(repo.clone())
        .or_insert_with(|| tree_lookup.tree_shas_at_head(&repo));
    match tree {
        Ok(shas) => match shas.get(folder_path) {
            Some(sha) if sha == &entry.skill_folder_hash => Currency::UpToDate,
            Some(_) => Currency::UpdateAvailable,
            None => Currency::Unknown,
        },
        Err(_) => Currency::Unknown,
    }
}

fn dotagents_currency(
    name: &str,
    ledger: &[DotagentsSkill],
    commit_lookup: &dyn CommitLookup,
) -> Currency {
    let Some(entry) = ledger.iter().find(|skill| skill.name == name) else {
        return Currency::Unknown;
    };
    let (Some(repo), Some(installed)) = (&entry.github_repo, &entry.installed_commit) else {
        return Currency::Unknown;
    };
    match commit_lookup.latest_commit(repo, &entry.path) {
        Ok(Some(latest)) if &latest == installed => Currency::UpToDate,
        Ok(Some(_)) => Currency::UpdateAvailable,
        Ok(None) | Err(_) => Currency::Unknown,
    }
}

fn plugin_currency(target: &OutdatedTarget, plugin_lookup: &dyn PluginManifestLookup) -> Currency {
    let Some((marketplace, plugin, installed_version)) = &target.plugin else {
        return Currency::Unknown;
    };
    let Some(installed) = installed_version else {
        return Currency::Unknown;
    };
    match plugin_lookup.marketplace_version(marketplace, plugin) {
        Ok(Some(latest)) if &latest == installed => Currency::UpToDate,
        Ok(Some(_)) => Currency::UpdateAvailable,
        Ok(None) | Err(_) => Currency::Unknown,
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
        fn latest_commit(&self, _repo: &str, _path: &str) -> Result<Option<String>, CoreError> {
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
        assert_eq!(result["write-tests"], Currency::UpdateAvailable);
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
        assert_eq!(result["write-tests"], Currency::UpToDate);
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
        assert_eq!(result["hand-placed"], Currency::NotTracked);
        assert_ne!(result["hand-placed"], Currency::UpToDate);
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
        assert_eq!(result["openai-templates"], Currency::UpdateAvailable);
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
        assert_eq!(result["write-tests"], Currency::Unknown);
    }
}
