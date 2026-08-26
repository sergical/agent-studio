// ============================================================================
// Skills Module - skill_update_check
// One searchable concept: check installed skills for upstream updates.
// Compares each dotagents/skills.sh skill's installed commit against the
// newest commit `gh api` reports for its path in the source repo, on a 6 h
// timer plus a manual "Check now". Results persist at
// `<app data>/skill-studio/update-check.json` so `skill_refresh::build_snapshot`
// can read them without shelling out on every rebuild. Read-only GitHub
// access via the user's own `gh` login; the app stores no tokens.
// ============================================================================

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use super::dotagents_ledger::{self, DotagentsSkill};
use super::lock_file;
use super::skill_agent_runner::{is_executable_file, pick_executable_line};
use super::skill_refresh;

/// How often the background loop re-checks for updates.
pub const UPDATE_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);

/// How long after startup the background loop waits before its first check,
/// so it doesn't compete with the initial skill scan for CPU/network.
const INITIAL_DELAY: Duration = Duration::from_secs(10);

/// How many `gh api` lookups run concurrently.
const LOOKUP_POOL_SIZE: usize = 4;

/// One skill's update-check result, persisted in `UpdateCheckStore`.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct SkillUpdateState {
    pub repo: String,
    pub path: String,
    pub installed_commit: Option<String>,
    pub latest_commit: Option<String>,
    pub latest_commit_at: Option<String>,
    pub checked_at: String,
    pub error: Option<String>,
    /// The skills.sh lock entry's `updatedAt` this state's `installed_commit`
    /// baseline was computed from, so a later run can tell whether the lock
    /// changed and the baseline needs re-querying. `None` for dotagents
    /// skills, which get `installed_commit` straight from `agents.lock`.
    #[serde(default)]
    pub lock_updated_at: Option<String>,
}

/// Result of `gh api ... commits`, or why it couldn't be run.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Default)]
#[serde(tag = "kind", content = "message", rename_all = "kebab-case")]
pub enum GhStatus {
    #[default]
    Ok,
    Missing,
    NotLoggedIn,
    Failed(String),
}

/// Everything one update check produced, persisted as-is at
/// `update_check_path`.
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct UpdateCheckStore {
    pub checked_at: Option<String>,
    pub gh_status: GhStatus,
    pub skills: BTreeMap<String, SkillUpdateState>,
}

/// The `SkillSnapshot.update_check` shape sent to the frontend: a flattened,
/// string-tagged view of `UpdateCheckStore` plus a ready-to-display count.
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UpdateCheckSummary {
    pub checked_at: Option<String>,
    pub gh_status: String, // "ok" | "missing" | "not-logged-in" | "failed"
    pub message: Option<String>,
    pub updates_available: u32,
}

impl Default for UpdateCheckSummary {
    fn default() -> Self {
        Self {
            checked_at: None,
            gh_status: "ok".to_string(),
            message: None,
            updates_available: 0,
        }
    }
}

/// `<app data>/skill-studio/update-check.json`.
pub fn update_check_path(app_data: &Path) -> PathBuf {
    app_data.join("skill-studio").join("update-check.json")
}

/// Read the persisted store, or a fresh empty one if it doesn't exist yet or
/// fails to parse. `app_data` is the app data dir, not the store file itself;
/// see `read_update_check_store_at` for a caller that already has the exact
/// file path.
pub fn read_update_check_store(app_data: &Path) -> UpdateCheckStore {
    read_update_check_store_at(&update_check_path(app_data))
}

/// Like `read_update_check_store`, but `path` is the exact store file path,
/// not the app data dir - for callers (like `skill_refresh`) that already
/// resolved `update_check_path` once and shouldn't have it joined again.
pub fn read_update_check_store_at(path: &Path) -> UpdateCheckStore {
    let Ok(content) = std::fs::read_to_string(path) else {
        return UpdateCheckStore::default();
    };
    serde_json::from_str(&content).unwrap_or_else(|e| {
        eprintln!(
            "skill update check: failed to parse {}: {e}",
            path.display()
        );
        UpdateCheckStore::default()
    })
}

/// Counter appended to `write_store`'s temp file name, so two writers (e.g.
/// a full check and a per-skill check that raced past the `UpdateCheckState`
/// guard) never pick the same temp path and clobber each other mid-write.
static WRITE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Write `store` atomically (unique temp file + rename) to `update_check_path`.
fn write_store(app_data: &Path, store: &UpdateCheckStore) -> Result<(), String> {
    let path = update_check_path(app_data);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create {}: {e}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(store)
        .map_err(|e| format!("Failed to serialize update check store: {e}"))?;
    let unique = WRITE_COUNTER.fetch_add(1, Ordering::SeqCst);
    let tmp_path = path.with_extension(format!("json.tmp.{}.{unique}", std::process::id()));
    std::fs::write(&tmp_path, json)
        .map_err(|e| format!("Failed to write {}: {e}", tmp_path.display()))?;
    std::fs::rename(&tmp_path, &path)
        .map_err(|e| format!("Failed to rename {}: {e}", tmp_path.display()))
}

/// True when `installed_commit` and `latest_commit` are both known and
/// differ.
pub fn has_update(state: &SkillUpdateState) -> bool {
    match (&state.installed_commit, &state.latest_commit) {
        (Some(installed), Some(latest)) => installed != latest,
        _ => false,
    }
}

/// Flatten `store` into the DTO the frontend reads off `SkillSnapshot`.
pub fn summarize(store: &UpdateCheckStore) -> UpdateCheckSummary {
    let (gh_status, message) = match &store.gh_status {
        GhStatus::Ok => ("ok", None),
        GhStatus::Missing => ("missing", None),
        GhStatus::NotLoggedIn => (
            "not-logged-in",
            Some("gh not logged in — run gh auth login".to_string()),
        ),
        GhStatus::Failed(m) => ("failed", Some(m.clone())),
    };
    let updates_available = store.skills.values().filter(|s| has_update(s)).count() as u32;
    UpdateCheckSummary {
        checked_at: store.checked_at.clone(),
        gh_status: gh_status.to_string(),
        message,
        updates_available,
    }
}

/// Looks up the newest commit that touched `path` in `repo`, so the real `gh`
/// implementation and a fake recorder can share one signature in tests.
/// Implementors must be `Sync`: `run_update_check` shares one `&dyn
/// CommitLookup` across a small worker pool.
pub trait CommitLookup: Sync {
    /// Returns `(sha, committer date)` for the newest commit touching `path`
    /// in `repo`, at or before `until` when given, or `Ok(None)` when the
    /// path has no commits (yet). `Err` messages from the real `gh`
    /// implementation may contain "gh auth login", which `run_update_check`
    /// treats as "not logged in" and stops on.
    fn latest_commit(
        &self,
        repo: &str,
        path: &str,
        until: Option<&str>,
    ) -> Result<Option<(String, String)>, String>;
}

/// Real `CommitLookup` backed by the `gh` CLI.
pub struct GhCommitLookup {
    pub gh_bin: PathBuf,
}

impl CommitLookup for GhCommitLookup {
    fn latest_commit(
        &self,
        repo: &str,
        path: &str,
        until: Option<&str>,
    ) -> Result<Option<(String, String)>, String> {
        let mut api_path = format!(
            "repos/{repo}/commits?path={}&per_page=1",
            urlencoding::encode(path)
        );
        if let Some(until) = until {
            api_path.push_str(&format!("&until={}", urlencoding::encode(until)));
        }

        let output = std::process::Command::new(&self.gh_bin)
            .arg("api")
            .arg(&api_path)
            .arg("--jq")
            .arg(".[0] | [.sha, .commit.committer.date] | @tsv")
            .output()
            .map_err(|e| format!("Failed to run gh: {e}"))?;

        if !output.status.success() {
            return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.trim();
        if line.is_empty() {
            return Ok(None);
        }
        let mut parts = line.splitn(2, '\t');
        let sha = parts.next().unwrap_or_default().to_string();
        let date = parts.next().unwrap_or_default().to_string();
        if sha.is_empty() {
            return Ok(None);
        }
        Ok(Some((sha, date)))
    }
}

/// Resolve `gh` on `$PATH` via a login shell, the same way
/// `skill_agent_runner::resolve_binary` finds harness binaries. `None` when
/// `gh` isn't installed.
pub(crate) fn resolve_gh_binary() -> Option<PathBuf> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
    let output = std::process::Command::new(&shell)
        .arg("-lc")
        .arg("command -v gh")
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    pick_executable_line(&stdout, is_executable_file)
}

/// True when an error message from `CommitLookup::latest_commit` indicates
/// the user isn't logged into `gh`.
fn is_not_logged_in(message: &str) -> bool {
    message.contains("gh auth login")
}

/// One repo/path pair worth checking, and how its `installed_commit` is
/// determined.
struct Candidate {
    name: String,
    repo: String,
    path: String,
    kind: CandidateKind,
}

enum CandidateKind {
    /// `installed_commit` comes straight from `agents.lock`.
    Dotagents { installed_commit: Option<String> },
    /// `installed_commit` is the newest commit at or before this lock
    /// entry's `updatedAt` - queried unless the cached baseline is still
    /// valid for the same `updatedAt`.
    SkillsSh { updated_at: String },
}

/// Build the candidate list from the dotagents ledger and the skills.sh lock
/// file under `home/.agents`, dotagents winning over skills.sh for a name
/// present in both (matches `provenance::SourceKind`'s precedence). Manual
/// and plugin skills have no ledger entry, so they're never candidates.
fn build_candidates(home: &Path) -> Vec<Candidate> {
    let agents_dir = home.join(".agents");

    // A fork's `base_commit` is the pinned "installed" side of the compare -
    // exactly the shape `CandidateKind::Dotagents` already models - and a
    // fork wins over a same-named ledger entry, same as dotagents wins over
    // skills.sh: it's the more specific, more recently established source.
    let fork_registry = super::skill_fork_registry::read_fork_registry_or_default(home);
    let mut candidates: Vec<Candidate> = fork_registry
        .forks
        .iter()
        .map(|(name, record)| Candidate {
            name: name.clone(),
            repo: record.repo.clone(),
            path: record.path.clone(),
            kind: CandidateKind::Dotagents {
                installed_commit: Some(record.base_commit.clone()),
            },
        })
        .collect();
    let fork_names: std::collections::BTreeSet<String> =
        fork_registry.forks.keys().cloned().collect();

    let dotagents_skills: Vec<DotagentsSkill> =
        dotagents_ledger::read_dotagents_ledger(&agents_dir).unwrap_or_else(|e| {
            eprintln!("skill update check: failed to read dotagents ledger: {e}");
            Vec::new()
        });
    let mut dotagents_names: std::collections::BTreeSet<String> =
        dotagents_skills.iter().map(|s| s.name.clone()).collect();
    dotagents_names.extend(fork_names.iter().cloned());

    candidates.extend(dotagents_skills.into_iter().filter_map(|s| {
        if fork_names.contains(&s.name) {
            return None; // fork wins
        }
        s.github_repo.map(|repo| Candidate {
            name: s.name,
            repo,
            path: s.path,
            kind: CandidateKind::Dotagents {
                installed_commit: s.installed_commit,
            },
        })
    }));

    let lock =
        lock_file::read_lock_file_at(&agents_dir.join(".skill-lock.json")).unwrap_or_else(|e| {
            eprintln!("skill update check: failed to read skill lock file: {e}");
            lock_file::SkillLockFile {
                version: 3,
                skills: HashMap::new(),
            }
        });
    for (name, entry) in lock.skills {
        if dotagents_names.contains(&name) {
            continue; // dotagents/fork wins
        }
        if entry.source_type != "github" {
            continue;
        }
        let Some(repo) = dotagents_ledger::github_repo_from_source(&entry.source) else {
            continue;
        };
        let Some(skill_path) = entry.skill_path else {
            continue;
        };
        let path = skill_path
            .strip_suffix("/SKILL.md")
            .unwrap_or(&skill_path)
            .to_string();
        let updated_at = if entry.updated_at.is_empty() {
            entry.installed_at
        } else {
            entry.updated_at
        };
        candidates.push(Candidate {
            name,
            repo,
            path,
            kind: CandidateKind::SkillsSh { updated_at },
        });
    }

    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    candidates
}

/// Check one candidate, given the previous run's state for it (if any).
/// Returns `None` when `stop` was already set before this candidate could be
/// looked up at all - the caller falls back to the previous state, if any.
fn check_candidate(
    candidate: &Candidate,
    previous: Option<&SkillUpdateState>,
    lookup: &dyn CommitLookup,
    stop: &AtomicBool,
    not_logged_in_message: &Mutex<Option<String>>,
    now: &str,
) -> Option<SkillUpdateState> {
    if stop.load(Ordering::SeqCst) {
        return None;
    }

    let mut error: Option<String> = None;
    let mut stopped = false;
    // Only set for `SkillsSh` candidates, and only once the baseline lookup
    // for `updated_at` actually succeeds (or was already cached for that
    // exact `updated_at`). A failed lookup falls back to the previous
    // installed_commit but must NOT record the new `updated_at` here, or the
    // next run's cache check would treat the stale fallback as a valid
    // baseline for `updated_at` forever and never retry the lookup.
    let mut lock_updated_at: Option<String> = None;

    let installed_commit = match &candidate.kind {
        CandidateKind::Dotagents { installed_commit } => installed_commit.clone(),
        CandidateKind::SkillsSh { updated_at } => {
            let cached = previous.filter(|p| {
                p.installed_commit.is_some() && p.lock_updated_at.as_deref() == Some(updated_at)
            });
            if let Some(cached) = cached {
                lock_updated_at = Some(updated_at.clone());
                cached.installed_commit.clone()
            } else {
                match lookup.latest_commit(&candidate.repo, &candidate.path, Some(updated_at)) {
                    Ok(found) => {
                        lock_updated_at = Some(updated_at.clone());
                        found.map(|(sha, _)| sha)
                    }
                    Err(e) if is_not_logged_in(&e) => {
                        stop.store(true, Ordering::SeqCst);
                        *not_logged_in_message
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(e);
                        stopped = true;
                        // Keep whatever baseline key (if any) the previous
                        // run recorded, so a retry happens once this stops
                        // short-circuiting.
                        lock_updated_at = previous.and_then(|p| p.lock_updated_at.clone());
                        previous.and_then(|p| p.installed_commit.clone())
                    }
                    Err(e) => {
                        error = Some(e);
                        lock_updated_at = previous.and_then(|p| p.lock_updated_at.clone());
                        previous.and_then(|p| p.installed_commit.clone())
                    }
                }
            }
        }
    };

    let (latest_commit, latest_commit_at) = if stopped {
        (
            previous.and_then(|p| p.latest_commit.clone()),
            previous.and_then(|p| p.latest_commit_at.clone()),
        )
    } else {
        match lookup.latest_commit(&candidate.repo, &candidate.path, None) {
            Ok(Some((sha, date))) => (Some(sha), Some(date)),
            Ok(None) => (None, None),
            Err(e) if is_not_logged_in(&e) => {
                stop.store(true, Ordering::SeqCst);
                *not_logged_in_message
                    .lock()
                    .unwrap_or_else(|e| e.into_inner()) = Some(e);
                (
                    previous.and_then(|p| p.latest_commit.clone()),
                    previous.and_then(|p| p.latest_commit_at.clone()),
                )
            }
            Err(e) => {
                error = Some(e);
                (
                    previous.and_then(|p| p.latest_commit.clone()),
                    previous.and_then(|p| p.latest_commit_at.clone()),
                )
            }
        }
    };

    Some(SkillUpdateState {
        repo: candidate.repo.clone(),
        path: candidate.path.clone(),
        installed_commit,
        latest_commit,
        latest_commit_at,
        checked_at: now.to_string(),
        error,
        lock_updated_at,
    })
}

/// Core of `run_update_check`/`run_update_check_for`: builds the candidate
/// list (optionally narrowed to `only`), checks each from a small worker
/// pool, and writes the result. When `only` is set, every other skill's
/// previously recorded state is carried over unchanged; a full run's result
/// is exactly the freshly checked candidates.
fn run_update_check_impl(
    home: &Path,
    app_data: &Path,
    lookup: &dyn CommitLookup,
    only: Option<&[String]>,
) -> UpdateCheckStore {
    let previous = read_update_check_store(app_data);
    let now = Utc::now().to_rfc3339();

    let mut candidates = build_candidates(home);
    if let Some(only) = only {
        candidates.retain(|c| only.contains(&c.name));
    }

    let stop = AtomicBool::new(false);
    let not_logged_in_message: Mutex<Option<String>> = Mutex::new(None);
    let queue: Mutex<VecDeque<Candidate>> = Mutex::new(candidates.into_iter().collect());
    let computed: Mutex<BTreeMap<String, SkillUpdateState>> = Mutex::new(BTreeMap::new());

    std::thread::scope(|scope| {
        for _ in 0..LOOKUP_POOL_SIZE {
            scope.spawn(|| loop {
                let next = queue.lock().unwrap_or_else(|e| e.into_inner()).pop_front();
                let Some(candidate) = next else { break };

                let prev_state = previous.skills.get(&candidate.name);
                if let Some(state) = check_candidate(
                    &candidate,
                    prev_state,
                    lookup,
                    &stop,
                    &not_logged_in_message,
                    &now,
                ) {
                    computed
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(candidate.name.clone(), state);
                } else if let Some(state) = prev_state {
                    // Stop was already set before this one could be looked
                    // up; keep whatever we knew about it before.
                    computed
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(candidate.name.clone(), state.clone());
                }
            });
        }
    });

    let gh_status = if not_logged_in_message.into_inner().unwrap_or(None).is_some() {
        GhStatus::NotLoggedIn
    } else {
        GhStatus::Ok
    };

    let computed = computed.into_inner().unwrap_or_else(|e| e.into_inner());
    let skills = if only.is_some() {
        let mut merged = previous.skills.clone();
        merged.extend(computed);
        merged
    } else {
        computed
    };

    let store = UpdateCheckStore {
        checked_at: Some(now),
        gh_status,
        skills,
    };
    if let Err(e) = write_store(app_data, &store) {
        eprintln!("skill update check: failed to write store: {e}");
    }
    store
}

/// Check every dotagents/skills.sh skill for an upstream update, using
/// `lookup` for the actual GitHub queries. Pure aside from the filesystem
/// reads/writes, so it's the unit under test.
pub fn run_update_check(
    home: &Path,
    app_data: &Path,
    lookup: &dyn CommitLookup,
) -> UpdateCheckStore {
    run_update_check_impl(home, app_data, lookup, None)
}

/// Like `run_update_check`, but only re-checks the skills named in `names`;
/// every other skill's previously recorded state is left as-is. Used after a
/// successful `update_skill` so that one skill's row updates immediately
/// without re-querying the other 59.
pub fn run_update_check_for(
    home: &Path,
    app_data: &Path,
    lookup: &dyn CommitLookup,
    names: &[String],
) -> UpdateCheckStore {
    run_update_check_impl(home, app_data, lookup, Some(names))
}

/// Resolve `gh`, then run the check for real - the production entry point
/// both `spawn_update_check_loop` and `check_skill_updates_now` call. When
/// `gh` isn't installed, writes `gh_status: Missing` without doing any
/// lookups (and without touching previously recorded skill states).
fn run_update_check_now(home: &Path, app_data: &Path) -> UpdateCheckStore {
    match resolve_gh_binary() {
        Some(gh_bin) => run_update_check(home, app_data, &GhCommitLookup { gh_bin }),
        None => {
            let previous = read_update_check_store(app_data);
            let store = UpdateCheckStore {
                checked_at: Some(Utc::now().to_rfc3339()),
                gh_status: GhStatus::Missing,
                skills: previous.skills,
            };
            if let Err(e) = write_store(app_data, &store) {
                eprintln!("skill update check: failed to write store: {e}");
            }
            store
        }
    }
}

/// Shared "a check is already running" guard, so the background loop and the
/// manual `check_skill_updates_now` command never run `gh api` concurrently.
#[derive(Clone, Default)]
pub struct UpdateCheckState {
    in_progress: std::sync::Arc<Mutex<bool>>,
}

impl UpdateCheckState {
    /// Attempts to claim the "in progress" flag; `false` when another check
    /// is already running.
    fn try_begin(&self) -> bool {
        let mut guard = self.in_progress.lock().unwrap_or_else(|e| e.into_inner());
        if *guard {
            false
        } else {
            *guard = true;
            true
        }
    }

    fn end(&self) {
        *self.in_progress.lock().unwrap_or_else(|e| e.into_inner()) = false;
    }
}

/// Run the check now, on the calling thread, then ask `skill_refresh` to
/// rebuild the snapshot so `has_update` reflects the result. Skips the run
/// (returning the last-known summary) when a check is already in flight.
pub fn check_now(app: &AppHandle, state: &UpdateCheckState) -> Result<UpdateCheckSummary, String> {
    let home = dirs::home_dir().ok_or("Could not find home directory")?;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not resolve app data dir: {e}"))?;

    if !state.try_begin() {
        return Ok(summarize(&read_update_check_store(&app_data)));
    }
    let store = run_update_check_now(&home, &app_data);
    state.end();

    skill_refresh::request_snapshot_rebuild(app);
    Ok(summarize(&store))
}

/// Re-check a single skill (after a successful `update_skill`) and request a
/// rebuild. Best-effort: errors are logged, never propagated, since this
/// runs after the update itself already succeeded. Shares `state`'s
/// "in progress" guard with `check_now`/`check_skill_updates_now`: if a full
/// check is already running, this skips its own `gh api` calls entirely
/// (rather than queuing behind it) and just requests a rebuild, since the
/// full check it's yielding to will cover this skill anyway.
pub fn check_now_for_skill(app: &AppHandle, state: &UpdateCheckState, skill_name: &str) {
    if !state.try_begin() {
        skill_refresh::request_snapshot_rebuild(app);
        return;
    }

    let Some(home) = dirs::home_dir() else {
        eprintln!("skill update check: could not find home directory");
        state.end();
        return;
    };
    let Ok(app_data) = app.path().app_data_dir() else {
        eprintln!("skill update check: could not resolve app data dir");
        state.end();
        return;
    };
    let names = [skill_name.to_string()];
    if let Some(gh_bin) = resolve_gh_binary() {
        run_update_check_for(&home, &app_data, &GhCommitLookup { gh_bin }, &names);
    } // else: gh_status stays whatever it already was; nothing to re-check
    state.end();
    skill_refresh::request_snapshot_rebuild(app);
}

/// Start the background loop on its own thread: waits `INITIAL_DELAY`, checks,
/// sleeps `UPDATE_CHECK_INTERVAL`, repeats for the app's lifetime. Registers
/// its `UpdateCheckState` as managed state so `check_skill_updates_now` shares
/// the same "in progress" guard.
pub fn spawn_update_check_loop(app: AppHandle) {
    let state = UpdateCheckState::default();
    app.manage(state.clone());

    std::thread::spawn(move || {
        std::thread::sleep(INITIAL_DELAY);
        loop {
            if let Err(e) = check_now(&app, &state) {
                eprintln!("skill update check: check failed: {e}");
            }
            std::thread::sleep(UPDATE_CHECK_INTERVAL);
        }
    });
}

/// Ask the background loop to run an update check right now, blocking the
/// calling (async command) thread until it finishes.
#[tauri::command]
pub async fn check_skill_updates_now(
    app: AppHandle,
    state: tauri::State<'_, UpdateCheckState>,
) -> Result<UpdateCheckSummary, String> {
    let state = state.inner().clone();
    tauri::async_runtime::spawn_blocking(move || check_now(&app, &state))
        .await
        .map_err(|e| format!("Update check task failed: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Mutex as StdMutex;

    /// One scripted answer to a `CommitLookup::latest_commit` call: the sha
    /// and commit date on a hit, or the error message on a failure.
    type LookupAnswer = Result<Option<(String, String)>, String>;

    /// Records every `latest_commit` call and returns scripted answers by
    /// call index, so tests can assert both "what was asked" and "what came
    /// back".
    #[derive(Default)]
    struct FakeLookup {
        calls: StdMutex<Vec<(String, String, Option<String>)>>,
        answers: StdMutex<VecDeque<LookupAnswer>>,
    }

    impl FakeLookup {
        fn with_answers(answers: Vec<LookupAnswer>) -> Self {
            Self {
                calls: StdMutex::new(Vec::new()),
                answers: StdMutex::new(answers.into_iter().collect()),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    impl CommitLookup for FakeLookup {
        fn latest_commit(
            &self,
            repo: &str,
            path: &str,
            until: Option<&str>,
        ) -> Result<Option<(String, String)>, String> {
            self.calls.lock().unwrap().push((
                repo.to_string(),
                path.to_string(),
                until.map(|s| s.to_string()),
            ));
            self.answers.lock().unwrap().pop_front().unwrap_or(Ok(None))
        }
    }

    fn write_agents_lock(home: &Path, name: &str, repo: &str, path: &str, commit: &str) {
        fs::create_dir_all(home.join(".agents")).unwrap();
        fs::write(
            home.join(".agents/agents.lock"),
            format!(
                r#"
[skills.{name}]
source = "{repo}"
resolved_path = "{path}"
resolved_commit = "{commit}"
"#
            ),
        )
        .unwrap();
    }

    fn write_skill_lock(home: &Path, name: &str, source: &str, skill_path: &str, updated_at: &str) {
        fs::create_dir_all(home.join(".agents")).unwrap();
        let json = serde_json::json!({
            "version": 3,
            "skills": {
                name: {
                    "source": source,
                    "sourceType": "github",
                    "sourceUrl": format!("https://github.com/{source}"),
                    "skillPath": skill_path,
                    "skillFolderHash": "abc",
                    "installedAt": "2026-01-01T00:00:00Z",
                    "updatedAt": updated_at,
                }
            }
        });
        fs::write(
            home.join(".agents/.skill-lock.json"),
            serde_json::to_string(&json).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn dotagents_newer_commit_has_update() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        write_agents_lock(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            "a".repeat(40).as_str(),
        );

        let lookup = FakeLookup::with_answers(vec![Ok(Some((
            "b".repeat(40),
            "2026-02-01T00:00:00Z".to_string(),
        )))]);
        let store = run_update_check(&home, &app_data, &lookup);

        let state = store.skills.get("find-bugs").unwrap();
        assert!(has_update(state));
        assert_eq!(
            state.installed_commit.as_deref(),
            Some("a".repeat(40).as_str())
        );
        assert_eq!(
            state.latest_commit.as_deref(),
            Some("b".repeat(40).as_str())
        );
    }

    #[test]
    fn dotagents_same_commit_has_no_update() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        let commit = "a".repeat(40);
        write_agents_lock(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &commit,
        );

        let lookup = FakeLookup::with_answers(vec![Ok(Some((
            commit.clone(),
            "2026-02-01T00:00:00Z".to_string(),
        )))]);
        let store = run_update_check(&home, &app_data, &lookup);

        let state = store.skills.get("find-bugs").unwrap();
        assert!(!has_update(state));
    }

    #[test]
    fn skills_sh_baseline_queried_with_until_then_latest() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        write_skill_lock(
            &home,
            "write-tests",
            "obra/write-tests",
            "apps/skills/extra/write-tests/SKILL.md",
            "2026-01-15T00:00:00Z",
        );

        let baseline_sha = "c".repeat(40);
        let latest_sha = "d".repeat(40);
        let lookup = FakeLookup::with_answers(vec![
            Ok(Some((
                baseline_sha.clone(),
                "2026-01-14T00:00:00Z".to_string(),
            ))),
            Ok(Some((
                latest_sha.clone(),
                "2026-02-01T00:00:00Z".to_string(),
            ))),
        ]);
        let store = run_update_check(&home, &app_data, &lookup);

        let calls = lookup.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].2, Some("2026-01-15T00:00:00Z".to_string()));
        assert_eq!(calls[1].2, None);
        drop(calls);

        let state = store.skills.get("write-tests").unwrap();
        assert_eq!(
            state.installed_commit.as_deref(),
            Some(baseline_sha.as_str())
        );
        assert_eq!(state.latest_commit.as_deref(), Some(latest_sha.as_str()));
        assert!(has_update(state));
    }

    #[test]
    fn skills_sh_baseline_reused_when_updated_at_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        write_skill_lock(
            &home,
            "write-tests",
            "obra/write-tests",
            "apps/skills/extra/write-tests/SKILL.md",
            "2026-01-15T00:00:00Z",
        );

        let baseline_sha = "c".repeat(40);
        let lookup1 = FakeLookup::with_answers(vec![
            Ok(Some((
                baseline_sha.clone(),
                "2026-01-14T00:00:00Z".to_string(),
            ))),
            Ok(Some((
                baseline_sha.clone(),
                "2026-01-14T00:00:00Z".to_string(),
            ))),
        ]);
        run_update_check(&home, &app_data, &lookup1);
        assert_eq!(lookup1.call_count(), 2); // baseline + latest, first run

        let lookup2 = FakeLookup::with_answers(vec![Ok(Some((
            baseline_sha.clone(),
            "2026-01-14T00:00:00Z".to_string(),
        )))]);
        run_update_check(&home, &app_data, &lookup2);
        // Only the "latest" query; the baseline is reused from the cache.
        assert_eq!(lookup2.call_count(), 1);
        assert_eq!(lookup2.calls.lock().unwrap()[0].2, None);
    }

    #[test]
    fn skills_sh_baseline_retries_after_updated_at_changes_and_lookup_fails() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        write_skill_lock(
            &home,
            "write-tests",
            "obra/write-tests",
            "apps/skills/extra/write-tests/SKILL.md",
            "2026-01-15T00:00:00Z",
        );

        let baseline_sha = "c".repeat(40);
        let lookup1 = FakeLookup::with_answers(vec![
            Ok(Some((
                baseline_sha.clone(),
                "2026-01-14T00:00:00Z".to_string(),
            ))),
            Ok(Some((
                baseline_sha.clone(),
                "2026-01-14T00:00:00Z".to_string(),
            ))),
        ]);
        run_update_check(&home, &app_data, &lookup1);

        // The lock's updatedAt moves on, and the baseline lookup for the new
        // updatedAt fails.
        write_skill_lock(
            &home,
            "write-tests",
            "obra/write-tests",
            "apps/skills/extra/write-tests/SKILL.md",
            "2026-02-15T00:00:00Z",
        );
        let lookup2 = FakeLookup::with_answers(vec![
            Err("network unreachable".to_string()),
            Ok(Some((
                baseline_sha.clone(),
                "2026-01-14T00:00:00Z".to_string(),
            ))),
        ]);
        let store2 = run_update_check(&home, &app_data, &lookup2);
        let state2 = store2.skills.get("write-tests").unwrap();
        // Stale baseline kept, but not recorded as valid for the new
        // updatedAt - and an error surfaces so the UI can show it.
        assert_eq!(
            state2.installed_commit.as_deref(),
            Some(baseline_sha.as_str())
        );
        assert_eq!(state2.error.as_deref(), Some("network unreachable"));
        assert_ne!(
            state2.lock_updated_at.as_deref(),
            Some("2026-02-15T00:00:00Z")
        );

        // Next run must retry the baseline lookup instead of trusting the
        // stale fallback forever.
        let lookup3 = FakeLookup::with_answers(vec![
            Ok(Some(("e".repeat(40), "2026-02-10T00:00:00Z".to_string()))),
            Ok(Some(("e".repeat(40), "2026-02-10T00:00:00Z".to_string()))),
        ]);
        run_update_check(&home, &app_data, &lookup3);
        assert_eq!(lookup3.call_count(), 2); // baseline retried + latest
        assert_eq!(
            lookup3.calls.lock().unwrap()[0].2,
            Some("2026-02-15T00:00:00Z".to_string())
        );
    }

    #[test]
    fn forked_skill_is_a_candidate_pinned_to_its_base_commit_and_wins_over_the_ledger() {
        use super::super::skill_fork_registry::{ForkRecord, ForkRegistry, OriginTool};

        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        // A ledger entry for the same name would normally win via dotagents,
        // but the fork must take precedence and use its own base_commit.
        write_agents_lock(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            "z".repeat(40).as_str(),
        );

        let mut registry = ForkRegistry::default();
        let base_commit = "a".repeat(40);
        registry.forks.insert(
            "find-bugs".to_string(),
            ForkRecord {
                forked_at: "2026-01-01T00:00:00Z".to_string(),
                origin_tool: OriginTool::Dotagents,
                origin_source: "getsentry/find-bugs".to_string(),
                repo: "getsentry/find-bugs".to_string(),
                path: "skills/find-bugs".to_string(),
                declared_ref: None,
                base_commit: base_commit.clone(),
            },
        );
        super::super::skill_fork_registry::write_fork_registry(&home, &registry).unwrap();

        let lookup = FakeLookup::with_answers(vec![Ok(Some((
            "b".repeat(40),
            "2026-02-01T00:00:00Z".to_string(),
        )))]);
        let store = run_update_check(&home, &app_data, &lookup);

        assert_eq!(lookup.call_count(), 1); // one candidate, not two
        let state = store.skills.get("find-bugs").unwrap();
        assert_eq!(
            state.installed_commit.as_deref(),
            Some(base_commit.as_str())
        );
        assert!(has_update(state));
    }

    #[test]
    fn non_github_source_is_skipped() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        fs::create_dir_all(home.join(".agents")).unwrap();
        let json = serde_json::json!({
            "version": 3,
            "skills": {
                "gitlab-skill": {
                    "source": "someorg/gitlab-skill",
                    "sourceType": "gitlab",
                    "sourceUrl": "https://gitlab.com/someorg/gitlab-skill",
                    "skillPath": "skill/SKILL.md",
                    "skillFolderHash": "abc",
                    "installedAt": "2026-01-01T00:00:00Z",
                    "updatedAt": "2026-01-01T00:00:00Z",
                }
            }
        });
        fs::write(
            home.join(".agents/.skill-lock.json"),
            serde_json::to_string(&json).unwrap(),
        )
        .unwrap();

        let lookup = FakeLookup::default();
        let store = run_update_check(&home, &app_data, &lookup);

        assert!(store.skills.is_empty());
        assert_eq!(lookup.call_count(), 0);
    }

    #[test]
    fn lookup_error_keeps_previous_commits_and_records_error() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        let commit = "a".repeat(40);
        write_agents_lock(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            &commit,
        );

        let previous_latest = "b".repeat(40);
        let seeded = UpdateCheckStore {
            checked_at: Some("2026-01-01T00:00:00Z".to_string()),
            gh_status: GhStatus::Ok,
            skills: BTreeMap::from([(
                "find-bugs".to_string(),
                SkillUpdateState {
                    repo: "getsentry/find-bugs".to_string(),
                    path: "skills/find-bugs".to_string(),
                    installed_commit: Some(commit.clone()),
                    latest_commit: Some(previous_latest.clone()),
                    latest_commit_at: Some("2026-01-01T00:00:00Z".to_string()),
                    checked_at: "2026-01-01T00:00:00Z".to_string(),
                    error: None,
                    lock_updated_at: None,
                },
            )]),
        };
        write_store(&app_data, &seeded).unwrap();

        let lookup = FakeLookup::with_answers(vec![Err("network unreachable".to_string())]);
        let store = run_update_check(&home, &app_data, &lookup);

        let state = store.skills.get("find-bugs").unwrap();
        assert_eq!(
            state.latest_commit.as_deref(),
            Some(previous_latest.as_str())
        );
        assert_eq!(state.error.as_deref(), Some("network unreachable"));
    }

    #[test]
    fn store_round_trips_through_the_file() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        write_agents_lock(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            "a".repeat(40).as_str(),
        );

        let lookup = FakeLookup::with_answers(vec![Ok(Some((
            "b".repeat(40),
            "2026-02-01T00:00:00Z".to_string(),
        )))]);
        run_update_check(&home, &app_data, &lookup);

        let reloaded = read_update_check_store(&app_data);
        let state = reloaded.skills.get("find-bugs").unwrap();
        assert_eq!(
            state.latest_commit.as_deref(),
            Some("b".repeat(40).as_str())
        );
        assert_eq!(reloaded.gh_status, GhStatus::Ok);
    }

    #[test]
    fn not_logged_in_short_circuits() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let app_data = tmp.path().join("data");
        write_agents_lock(
            &home,
            "find-bugs",
            "getsentry/find-bugs",
            "skills/find-bugs",
            "a".repeat(40).as_str(),
        );

        let lookup = FakeLookup::with_answers(vec![Err(
            "gh: To get started with GitHub CLI, run: gh auth login".to_string(),
        )]);
        let store = run_update_check(&home, &app_data, &lookup);

        assert_eq!(store.gh_status, GhStatus::NotLoggedIn);
    }

    #[test]
    fn update_check_state_guard_rejects_second_concurrent_begin() {
        let state = UpdateCheckState::default();
        assert!(state.try_begin());
        assert!(
            !state.try_begin(),
            "a second begin must be rejected while the first is in progress"
        );
        state.end();
        assert!(
            state.try_begin(),
            "begin must succeed again once the first ends"
        );
    }

    #[test]
    fn concurrent_writers_use_distinct_temp_files_and_dont_clobber() {
        let tmp = tempfile::tempdir().unwrap();
        let app_data = tmp.path().to_path_buf();

        // Two writers racing `write_store` (e.g. a full check and a
        // per-skill check that both got past the guard) must each get their
        // own temp file, so neither's partial write can land in the other's
        // rename.
        std::thread::scope(|scope| {
            for i in 0..8 {
                let app_data = app_data.clone();
                scope.spawn(move || {
                    let store = UpdateCheckStore {
                        checked_at: Some(format!("run-{i}")),
                        gh_status: GhStatus::Ok,
                        skills: BTreeMap::new(),
                    };
                    write_store(&app_data, &store).unwrap();
                });
            }
        });

        // The store file is valid JSON left by whichever writer finished
        // last - not a mix of two half-written payloads.
        let final_store = read_update_check_store(&app_data);
        assert!(final_store
            .checked_at
            .as_deref()
            .is_some_and(|c| c.starts_with("run-")));

        // No leftover temp files: every writer's rename succeeded.
        let leftover_temp_files = std::fs::read_dir(app_data.join("skill-studio"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .count();
        assert_eq!(leftover_temp_files, 0);
    }
}
