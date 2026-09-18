// ============================================================================
// Skills Module - skill_install
// `add_skill`'s new home: a thin adapter over `skill_studio_core::ops::install`
// (unit 3.5c). Every method - `Copy`, `Dotagents`, `SkillsSh` - funnels
// through that one op, run inside `tauri::async_runtime::spawn_blocking` so
// a `gh` fetch, an `npx` shell-out, or a file copy never sits on the UI
// task, the same shape `harness_first_run.rs`'s `detect_with_runtime`
// already uses. `install_preferences` is the second, much smaller adapter:
// it reads the same saved-preference/environment-default fact
// `add_method_defaults.rs` used to compute locally.
//
// Old path deleted in this unit: `skill_add.rs`'s own `add_via_copy`/
// `add_via_dotagents`/`add_via_skills_sh` (direct `std::fs`/`npx` calls,
// duplicating what `ops::install` and `ops_install_cli.rs` now own) and
// `skill_install_plan.rs` (superseded by `ops_install_cli::cli_args_and_cwd`,
// except the one unrelated Un-fork argv builder now living in
// `skill_fork.rs`).
//
// `SkillDestination::PerHarness` stays on the wire (see the module's own
// doc, `AddSkillSheet.tsx` builds it directly and is out of scope here), but
// this adapter treats it identically to `Universal`: `ops::install` only
// ever writes the one shared root, so a `PerHarness` request still gets a
// Universal write, linked into exactly the harnesses `request.agents` names
// - the same disk shape `docs/action-map/install.md`'s desired state
// describes for every method, not just Copy.
// ============================================================================

use std::path::{Path, PathBuf};

use skill_studio_core::dto::{InstallFile, InstallMethod, InstallOutcome, InstallRequest};
use skill_studio_core::identity::{
    CorrelationId, ProjectRef, RootScope, SkillName, UNIVERSAL_ROOT_RELATIVE,
};
use skill_studio_core::ops::{self, Operation, ResultEnvelope};
use skill_studio_core::ports::{OpContext, Runtime};

use super::agents::AgentId;
use super::github_skill_listing::GithubSkillEntry;
use super::skill_deployment::{deployment_id, SkillDestination};
use super::skill_dto::{
    AddSkillRequest, AddSkillResult, AddSkillsRequest, InstallScope, ParsedSkillSource,
    ParsedSkillSourceKind,
};
use super::skill_fork::{RepoSnapshot, UpstreamFetch};
use super::skill_fork_registry::{AddMethod, TrialScope};
use super::skill_trial;
use super::skill_trust_policy::{
    normalize_dotagents_source_identity, UNTRUSTED_DOTAGENTS_SOURCE_MESSAGE,
    UNTRUSTED_DOTAGENTS_SOURCE_PREFIX,
};
use super::skill_update_check::CommitLookup;

/// `AddMethod` (the desktop's own request enum) -> `InstallMethod` (the
/// op's). Same three cases, by name - `ops_install_cli.rs` and CLI's own
/// `From<clap's AddMethod> for InstallMethod` are the model.
fn core_method(method: AddMethod) -> InstallMethod {
    match method {
        AddMethod::Copy => InstallMethod::Copy,
        AddMethod::Dotagents => InstallMethod::Dotagents,
        AddMethod::SkillsSh => InstallMethod::SkillsSh,
    }
}

/// `AgentId` (the desktop's catalog id) -> `skill_studio_core::identity::AgentId`
/// (the op's harness newtype), by the catalog's own CLI name string - the
/// same conversion `set_harness_enabled`'s desktop adapter already uses.
fn core_harness(agent: AgentId) -> Result<skill_studio_core::identity::AgentId, String> {
    skill_studio_core::identity::AgentId::parse_harness(agent.cli_name()).map_err(|e| e.message)
}

/// Moved from `skill_add.rs`'s `derive_copy_name`, unchanged: `skill_name`
/// wins when the sheet set one; otherwise the source's own last path
/// segment. `Dotagents`/`SkillsSh` always require an explicit `skill_name`
/// already (`AddSkillSheet.tsx` fills it before submit), so this mainly
/// matters for a bare Copy request.
fn derive_name(source: &ParsedSkillSource) -> Result<String, String> {
    if let Some(name) = &source.skill_name {
        if !name.is_empty() {
            return Ok(name.clone());
        }
    }
    match source.kind {
        ParsedSkillSourceKind::Github => source
            .path
            .as_deref()
            .and_then(|p| p.rsplit('/').next())
            .map(std::string::ToString::to_string)
            .or_else(|| {
                source
                    .repo
                    .as_deref()
                    .and_then(|r| r.rsplit('/').next())
                    .map(std::string::ToString::to_string)
            })
            .ok_or_else(|| "Could not determine a skill name".to_string()),
        ParsedSkillSourceKind::Local => source
            .local_path
            .as_deref()
            .and_then(|p| Path::new(p).file_name())
            .and_then(|s| s.to_str())
            .map(std::string::ToString::to_string)
            .ok_or_else(|| "Could not determine a skill name".to_string()),
        ParsedSkillSourceKind::Git => Err("Copy is not supported for git sources".to_string()),
    }
}

/// `path`'s lexically-existing prefix, canonicalized, plus the remaining
/// (not-yet-created) components appended back on - the deleted
/// `skill_add.rs`'s `resolve_existing_path_prefix`, moved here unchanged.
/// A Copy destination normally doesn't exist yet, so a plain
/// `fs::canonicalize` would fail on it; this instead resolves as far as the
/// filesystem allows (following any symlink on the way, notably a
/// symlinked `.agents` or `.claude`) and reattaches the rest lexically.
fn resolve_existing_prefix(path: &Path) -> std::io::Result<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut missing = Vec::new();
    loop {
        match std::fs::canonicalize(&existing) {
            Ok(canonical) => {
                let mut resolved = canonical;
                for part in missing.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = existing.file_name().map(std::ffi::OsStr::to_os_string) else {
                    return Err(error);
                };
                existing.pop();
                missing.push(name);
            }
            Err(error) => return Err(error),
        }
    }
}

fn path_is_within(path: &Path, directory: &Path) -> bool {
    path == directory || path.starts_with(directory)
}

/// Refuses a local Copy `source` that is, or resolves through a symlink to,
/// the destination itself or an ancestor of `scope_root`/`destination` -
/// moved from `skill_add.rs`'s `validate_local_copy_destinations`, restated
/// from the source's own point of view per the review's item 1. Runs before
/// `read_skill_files` reads a single byte, so a source that would otherwise
/// leak the whole home directory or project root into a skill folder is
/// rejected up front. Returns the source's own canonical path for the caller
/// to walk.
fn guard_local_copy_source(
    source: &Path,
    scope_root: &Path,
    destination: &Path,
) -> Result<PathBuf, String> {
    let canonical_source = std::fs::canonicalize(source)
        .map_err(|e| format!("Could not resolve {}: {e}", source.display()))?;
    for candidate in [scope_root, destination] {
        let resolved = resolve_existing_prefix(candidate)
            .map_err(|e| format!("Could not resolve {}: {e}", candidate.display()))?;
        if path_is_within(&resolved, &canonical_source) {
            return Err(format!(
                "Local Copy source must not be the destination or an ancestor of it: {}",
                canonical_source.display()
            ));
        }
    }
    Ok(canonical_source)
}

/// Reads `dir` into the `InstallFile` list `InstallMethod::Copy` stages -
/// same walk as the CLI's own `read_skill_files` (`apps/cli/src/main.rs`),
/// duplicated rather than shared across the crate boundary the CLI binary
/// and the desktop crate don't otherwise cross.
fn read_skill_files(dir: &Path) -> Result<Vec<InstallFile>, String> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<InstallFile>) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                walk(root, &path, out)?;
            } else if file_type.is_file() {
                let contents = std::fs::read(&path)?;
                let relative_path = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                out.push(InstallFile {
                    relative_path,
                    contents,
                });
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out).map_err(|e| format!("Could not read {}: {e}", dir.display()))?;
    Ok(out)
}

/// `Copy`-method file gathering: a `Local` source is walked directly (no
/// network), a `Github` source is fetched into a scratch `tempdir` - once,
/// via `snapshot` when a batch install already opened one, otherwise one
/// `fetch_skill_dir` call - then walked the same way. `ops::install` stages
/// and swaps the resulting bytes atomically; this function's only job is
/// turning a source into the `Vec<InstallFile>` it stages from.
///
/// `pub(crate)`: `skill_add_operation.rs`'s batch worker shares this with
/// `add_skill_with_runtime` below, rather than each maintaining its own copy.
///
/// `scope_root`/`destination` are only used by the `Local` branch, to run
/// `guard_local_copy_source` before any read - see that function's doc.
pub(crate) fn gather_copy_files(
    source: &ParsedSkillSource,
    scope_root: &Path,
    destination: &Path,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
    snapshot: Option<&dyn RepoSnapshot>,
) -> Result<Vec<InstallFile>, String> {
    match source.kind {
        ParsedSkillSourceKind::Local => {
            let path = source
                .local_path
                .as_deref()
                .ok_or("A local source needs a path")?;
            let canonical = guard_local_copy_source(Path::new(path), scope_root, destination)?;
            read_skill_files(&canonical)
        }
        ParsedSkillSourceKind::Github => {
            let repo = source.repo.clone().ok_or("A GitHub source needs a repo")?;
            let path = source.path.clone().unwrap_or_default();
            let staging =
                tempfile::tempdir().map_err(|e| format!("Could not create a scratch dir: {e}"))?;
            if let Some(snapshot) = snapshot {
                snapshot.copy_dir(&path, staging.path())?;
            } else {
                let commit = match &source.git_ref {
                    Some(r) => r.clone(),
                    None => lookup
                        .latest_commit(&repo, &path, None)?
                        .map(|(sha, _)| sha)
                        .ok_or("Could not determine the skill's latest commit")?,
                };
                fetch.fetch_skill_dir(&repo, &path, &commit, staging.path())?;
            }
            read_skill_files(staging.path())
        }
        ParsedSkillSourceKind::Git => Err("Copy is not supported for git sources".to_string()),
    }
}

/// One batch entry's request, built from the batch's shared source plus the
/// entry's own folder/name - moved from `skill_add.rs`'s `request_for_entry`,
/// unchanged. `skill_add_operation.rs`'s batch worker loops this per
/// `GithubSkillEntry` rather than threading a second request shape through
/// `ops::install`.
pub(crate) fn request_for_entry(
    batch: &AddSkillsRequest,
    entry: &GithubSkillEntry,
) -> AddSkillRequest {
    let mut source = batch.source.clone();
    source.path = Some(entry.path.clone()).filter(|p| !p.is_empty());
    source.skill_name = Some(entry.name.clone());
    AddSkillRequest {
        source,
        method: batch.method,
        destination: batch.destination,
        agents: batch.agents.clone(),
        disabled_harnesses: batch.disabled_harnesses.clone(),
        scope: batch.scope,
        project_path: batch.project_path.clone(),
        trial: batch.trial,
    }
}

/// Downloads a Copy batch's shared repo once so `gather_copy_files` can copy
/// each entry's folder out of the same snapshot instead of refetching -
/// moved from `skill_add.rs`'s `open_repo_snapshot`. Non-Copy methods and a
/// fetcher with no bulk mode (`open_repo` returning `Ok(None)`) both fall
/// back to one `fetch_skill_dir` call per entry inside `gather_copy_files`.
pub(crate) fn open_batch_snapshot(
    request: &AddSkillsRequest,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
) -> Result<Option<Box<dyn RepoSnapshot>>, String> {
    if !matches!(request.method, AddMethod::Copy) {
        return Ok(None);
    }
    let repo = request
        .source
        .repo
        .clone()
        .ok_or("A GitHub source needs a repo")?;
    let path = request.source.path.clone().unwrap_or_default();
    let commit = match &request.source.git_ref {
        Some(r) => r.clone(),
        None => lookup
            .latest_commit(&repo, &path, None)?
            .map(|(sha, _)| sha)
            .ok_or_else(|| format!("Could not determine {repo}'s latest commit"))?,
    };
    fetch.open_repo(&repo, &commit)
}

/// The `Dotagents`/`SkillsSh` source argument `ops::install` passes on to
/// `ops_install_cli::cli_args_and_cwd` as `req.source` - `Github` and `Git`
/// both resolve to a plain string the CLI accepts as `add <source>`; `Local`
/// never reaches a CLI-shelling method (the sheet only offers Copy for a
/// local path).
fn cli_source_arg(source: &ParsedSkillSource) -> Option<String> {
    match source.kind {
        ParsedSkillSourceKind::Github => source.repo.clone(),
        ParsedSkillSourceKind::Git => source.url.clone(),
        ParsedSkillSourceKind::Local => None,
    }
}

/// The validated skill directory name a request will install under -
/// shared by `build_install_request` and `install_one`'s pre-read local
/// Copy guard, which needs the same name to compute the destination the
/// guard checks the source against before `build_install_request` runs.
fn derive_and_validate_name(request: &AddSkillRequest) -> Result<String, String> {
    let name = derive_name(&request.source)?;
    Ok(super::skill_agent_runner::validate_skill_dir_name(&name)?.to_string())
}

/// `request.scope`'s filesystem root (home or project) - shared by
/// `build_install_request` and `install_one`'s pre-read local Copy guard.
fn request_scope_root(request: &AddSkillRequest, rt: &Runtime) -> Result<PathBuf, String> {
    match request.scope {
        InstallScope::Global => Ok(rt.scope.home.lexical.clone()),
        InstallScope::Project => Ok(PathBuf::from(
            request
                .project_path
                .clone()
                .ok_or("Project scope needs a project path")?,
        )),
    }
}

/// Builds the op's own request from the desktop's wire request plus the
/// files a `Copy` install already gathered. `destination` is read but not
/// otherwise threaded through: see the module doc on `PerHarness`.
///
/// `pub(crate)`: shared with `skill_add_operation.rs`'s batch worker.
pub(crate) fn build_install_request(
    request: &AddSkillRequest,
    files: Vec<InstallFile>,
) -> Result<InstallRequest, String> {
    let name = derive_and_validate_name(request)?;
    let harnesses = request
        .agents
        .iter()
        .copied()
        .map(core_harness)
        .collect::<Result<Vec<_>, _>>()?;
    let scope = match request.scope {
        InstallScope::Global => RootScope::Global,
        InstallScope::Project => RootScope::Project(ProjectRef(PathBuf::from(
            request
                .project_path
                .clone()
                .ok_or("Project scope needs a project path")?,
        ))),
    };
    // `ops::install` only ever derives a Dotagents identity from `req.source`
    // itself (never a caller-set `trust_identity`, see that op's own doc);
    // `Copy`/`SkillsSh` have no such built-in gate, so this is the only place
    // either one reaches the trust prompt - the Skill Store's `skills-sh`
    // install and "Promote to global"'s `copy` install both need this to see
    // `NeedsTrust` for an untrusted GitHub/git source (review item 4).
    let trust_identity = match request.method {
        AddMethod::Copy | AddMethod::SkillsSh => {
            normalize_dotagents_source_identity(&request.source)
        }
        AddMethod::Dotagents => None,
    };
    Ok(InstallRequest {
        skill: SkillName(name),
        method: core_method(request.method),
        scope,
        harnesses,
        files,
        source: cli_source_arg(&request.source),
        trust_identity,
        trust_confirmed: false,
        save_as_preference: true,
    })
}

/// `ops::install`'s two outcomes, reshaped for the two callers that need to
/// tell them apart: `add_skill` (surfaces `NeedsTrust` as a plain error -
/// see the module doc) and the operation worker (surfaces it as a
/// structured `AddSkillUntrustedSource` phase event instead).
pub(crate) enum InstallAdapterOutcome {
    Result(AddSkillResult),
    NeedsTrust { identity: String },
}

/// The plain-error text `add_skill` returns for `NeedsTrust`: the
/// `SkillStoreInstallFlow`/"Promote to global" call sites (decision in
/// `launch-3-5c.md`: "go through the same adapter") have no operation event
/// stream to read a structured `NeedsTrust` from, so this string - built
/// from the same `UNTRUSTED_DOTAGENTS_SOURCE_MESSAGE` copy the operation
/// flow's toast already uses - is this unit's whole answer for them. Real
/// interactive retry UI for these two entry points is deferred; see
/// `issue-3.5c-followup-a.md`.
pub(crate) fn needs_trust_message(identity: &str) -> String {
    format!(
        "{UNTRUSTED_DOTAGENTS_SOURCE_PREFIX}: {UNTRUSTED_DOTAGENTS_SOURCE_MESSAGE} ({identity})"
    )
}

/// Turns the op's outcome into the sheet's `AddSkillResult`, running the
/// `trial`/`disabled_harnesses` follow-ups `ops::install` does not own
/// (decision 2, `launch-3-5c.md`): both run after the op's own write
/// succeeds, inside the same `spawn_blocking` task as the install itself.
/// A follow-up failure becomes `warning`, not an error - the install already
/// succeeded and the skill is on disk and usable.
///
/// `pub(crate)`: shared with `skill_add_operation.rs`'s batch worker.
pub(crate) fn finish_install(
    rt: &Runtime,
    request: &AddSkillRequest,
    outcome: InstallOutcome,
) -> InstallAdapterOutcome {
    let (skill, deployment_path, linked_harnesses) = match outcome {
        InstallOutcome::Installed {
            skill,
            deployment_path,
            linked_harnesses,
            ..
        } => (skill, deployment_path, linked_harnesses),
        InstallOutcome::NeedsTrust { identity } => {
            return InstallAdapterOutcome::NeedsTrust { identity };
        }
    };

    let mut deployments_created = vec![deployment_path.to_string_lossy().into_owned()];
    let claude_link_path = linked_harnesses
        .iter()
        .any(|h| h.as_str() == skill_studio_core::identity::AgentId::CLAUDE_CODE)
        .then(|| {
            let root = match &request.scope {
                InstallScope::Global => rt.scope.home.lexical.clone(),
                InstallScope::Project => {
                    PathBuf::from(request.project_path.clone().unwrap_or_default())
                }
            };
            root.join(".claude").join("skills").join(&skill.0)
        });
    if let Some(link) = &claude_link_path {
        deployments_created.push(link.to_string_lossy().into_owned());
    }

    let mut warnings = Vec::new();
    let home = rt.scope.home.lexical.clone();
    if request.trial {
        let deployment_id = deployment_id(
            &skill.0,
            match request.scope {
                InstallScope::Global => "global",
                InstallScope::Project => "project",
            },
            SkillDestination::Universal,
            "universal",
            request.project_path.as_deref(),
            &deployment_path,
        );
        let scope = match request.scope {
            InstallScope::Global => TrialScope::Global,
            InstallScope::Project => TrialScope::Project,
        };
        let write_lease = super::write_lease::WriteLease::default();
        match write_lease.try_acquire(&home) {
            Ok(guard) => {
                if let Err(e) = skill_trial::record_trial(
                    &guard,
                    &home,
                    &deployment_id,
                    scope,
                    request.project_path.as_deref(),
                    request.method,
                    deployment_path.clone(),
                    claude_link_path.clone(),
                    chrono::Utc::now(),
                ) {
                    warnings.push(format!("trial: {e}"));
                }
            }
            Err(e) => warnings.push(format!("trial: {e}")),
        }
    }
    for agent in &request.disabled_harnesses {
        if let Err(e) = disable_harness(rt, &skill.0, *agent, request.project_path.as_deref()) {
            warnings.push(format!("{}: {e}", agent.cli_name()));
        }
    }

    let tool = match request.method {
        AddMethod::Copy => "copy",
        AddMethod::Dotagents => "dotagents",
        AddMethod::SkillsSh => "skills-sh",
    };
    InstallAdapterOutcome::Result(AddSkillResult {
        name: skill.0,
        tool: tool.to_string(),
        command: format!("ops::install ({tool})"),
        deployments_created,
        warning: (!warnings.is_empty()).then(|| warnings.join("; ")),
    })
}

/// One `ops::set_harness_enabled(enabled: false)` call per
/// `disabled_harnesses` entry - directly, not through the legacy
/// `set_harness_enabled_with`/`set_new_universal_reader_enabled` dispatch
/// `skill_harness_disable.rs`'s own command still uses for a user-driven
/// toggle (decision 2).
fn disable_harness(
    rt: &Runtime,
    skill: &str,
    agent: AgentId,
    project_path: Option<&str>,
) -> Result<(), String> {
    let harness = core_harness(agent)?;
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let req = skill_studio_core::dto::SetHarnessEnabledRequest {
        skill: SkillName(skill.to_string()),
        harness,
        enabled: false,
        project_path: project_path.map(PathBuf::from),
    };
    ops::set_harness_enabled(rt, &ctx, &req)
        .map(|_| ())
        .map_err(|e| e.message)
}

// Review item 11: `add_skill_runs_on_a_blocking_thread_...` only proved
// where the injected `build_runtime` closure ran, not where `install_one`
// itself ran - a future edit could move the `install_one` call out of
// `spawn_blocking` and leave that test green. This thread-local is a
// same-thread relay: a test sets it from inside its `build_runtime` closure
// (which already runs on the `spawn_blocking` worker thread), and
// `install_one`, called synchronously afterward on that same OS thread,
// reads it back and records the thread it is actually running on. Moving
// `install_one` to a different thread than `build_runtime` leaves the probe
// unset, which the test treats as a failure. `#[cfg(test)]` only - zero
// cost and no surface in production.
#[cfg(test)]
thread_local! {
    static INSTALL_ONE_THREAD_PROBE: std::cell::RefCell<Option<std::sync::Arc<std::sync::Mutex<Option<std::thread::ThreadId>>>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn set_install_one_thread_probe(
    probe: std::sync::Arc<std::sync::Mutex<Option<std::thread::ThreadId>>>,
) {
    INSTALL_ONE_THREAD_PROBE.with(|cell| *cell.borrow_mut() = Some(probe));
}

/// One skill through `ops::install`: gather `Copy` files (if applicable,
/// against a batch's shared `snapshot` when given), build the op's request,
/// call `ops::install`, then run `finish_install`'s follow-ups. Shared by
/// `add_skill_with_runtime` below and `skill_add_operation.rs`'s single and
/// batch workers, so there is exactly one place that calls `ops::install`.
pub(crate) fn install_one(
    rt: &Runtime,
    request: &AddSkillRequest,
    fetch: &dyn UpstreamFetch,
    lookup: &dyn CommitLookup,
    snapshot: Option<&dyn RepoSnapshot>,
) -> Result<InstallAdapterOutcome, String> {
    #[cfg(test)]
    INSTALL_ONE_THREAD_PROBE.with(|cell| {
        if let Some(probe) = cell.borrow().as_ref() {
            *probe
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) =
                Some(std::thread::current().id());
        }
    });
    let files = if matches!(request.method, AddMethod::Copy) {
        let name = derive_and_validate_name(request)?;
        let scope_root = request_scope_root(request, rt)?;
        let destination = scope_root.join(UNIVERSAL_ROOT_RELATIVE).join(&name);
        gather_copy_files(
            &request.source,
            &scope_root,
            &destination,
            fetch,
            lookup,
            snapshot,
        )?
    } else {
        Vec::new()
    };
    let install_req = build_install_request(request, files)?;
    let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
    let result = ops::install(rt, &ctx, &install_req);
    let envelope = ResultEnvelope::from_result(Operation::Install, &rt.scope, &ctx, result);
    let outcome = super::core_runtime::to_command_result(envelope)?;
    Ok(finish_install(rt, request, outcome))
}

/// The GitHub-facing pair `Copy` needs, same shape `skill_add.rs`'s
/// `resolve_fetch_and_lookup` used - boxed so a real `add_skill` and a test
/// can hand this function the same signature.
pub(crate) type GithubTools = (Box<dyn UpstreamFetch + Send>, Box<dyn CommitLookup + Send>);

/// The command body, run inside `spawn_blocking` - `build_runtime` is
/// injectable so a unit test can prove this whole call, including the
/// runtime build and `ops::install`'s own CLI shell-out, never runs on the
/// calling task (`harness_first_run.rs::detect_with_runtime`'s pattern).
pub(crate) async fn add_skill_with_runtime(
    build_runtime: impl FnOnce() -> Result<Runtime, String> + Send + 'static,
    request: AddSkillRequest,
    github: GithubTools,
) -> Result<AddSkillResult, String> {
    let joined = tauri::async_runtime::spawn_blocking(move || {
        let rt = build_runtime()?;
        let (fetch, lookup) = github;
        match install_one(&rt, &request, fetch.as_ref(), lookup.as_ref(), None)? {
            InstallAdapterOutcome::Result(result) => Ok(result),
            InstallAdapterOutcome::NeedsTrust { identity } => Err(needs_trust_message(&identity)),
        }
    })
    .await;
    crate::timing_log::join_result_to_err("add_skill", joined)
}

/// The GitHub-facing pair both add commands run with: the real `gh`-backed
/// implementations, or ones that fail with "Run Check now first" when `gh`
/// isn't resolvable. Moved from `skill_add.rs`, unchanged.
pub(crate) fn resolve_fetch_and_lookup(app: &tauri::AppHandle) -> Result<GithubTools, String> {
    use tauri::Manager;
    let app_data = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not resolve app data dir: {e}"))?;
    Ok(
        if let Some(gh_bin) = super::skill_update_check::resolve_gh_binary() {
            (
                Box::new(super::skill_fork::RealUpstreamFetch {
                    gh_bin: gh_bin.clone(),
                    cache_dir: app_data.join("skill-studio").join("cache"),
                }),
                Box::new(super::skill_update_check::GhCommitLookup { gh_bin }),
            )
        } else {
            let message = "Run Check now first".to_string();
            (
                Box::new(Unavailable(message.clone())),
                Box::new(Unavailable(message)),
            )
        },
    )
}

struct Unavailable(String);

impl CommitLookup for Unavailable {
    fn latest_commit(
        &self,
        _repo: &str,
        _path: &str,
        _until: Option<&str>,
    ) -> Result<Option<(String, String)>, String> {
        Err(self.0.clone())
    }
}

impl UpstreamFetch for Unavailable {
    fn fetch_skill_dir(
        &self,
        _repo: &str,
        _path: &str,
        _commit: &str,
        _into: &Path,
    ) -> Result<(), String> {
        Err(self.0.clone())
    }
}

#[tauri::command]
pub async fn add_skill(
    request: AddSkillRequest,
    app: tauri::AppHandle,
) -> Result<AddSkillResult, String> {
    let github = resolve_fetch_and_lookup(&app)?;
    crate::timing_log::time_command_async(
        &app,
        "add_skill",
        add_skill_with_runtime(super::core_runtime::build_runtime_write, request, github),
    )
    .await
}

/// The saved or environment-default install method/harnesses, so
/// `AddSkillSheet.tsx` can pre-fill the second install the way it already
/// pre-fills the first from `add_method_defaults.rs`. `add_method_defaults.rs`
/// stays as-is: `ops::install_preferences` falls back to the exact same
/// environment default (`npx` on `PATH` -> `SkillsSh`, else `Copy`) when
/// nothing has been saved yet, so nothing downstream needs to reconcile two
/// different defaults.
#[tauri::command]
pub async fn install_preferences(
    scope: InstallScope,
    project_path: Option<String>,
    app: tauri::AppHandle,
) -> Result<skill_studio_core::dto::InstallPreferences, String> {
    crate::timing_log::time_command_blocking(&app, "install_preferences", move || {
        let rt = super::core_runtime::build_runtime_write()?;
        let root_scope = match scope {
            InstallScope::Global => RootScope::Global,
            InstallScope::Project => RootScope::Project(ProjectRef(PathBuf::from(
                project_path.ok_or("Project scope needs a project path")?,
            ))),
        };
        let ctx = OpContext::uncancellable(CorrelationId(ulid::Ulid::new().to_string()));
        let result = ops::install_preferences(&rt, &ctx, &root_scope);
        let envelope =
            ResultEnvelope::from_result(Operation::InstallPreferences, &rt.scope, &ctx, result);
        super::core_runtime::to_command_result(envelope)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use skill_studio_core::harness::HarnessCatalog;
    use skill_studio_core::ports::{Ports, Runtime};
    use skill_studio_core::RuntimeScope;
    use std::sync::Arc;

    fn test_runtime(home: &Path) -> Runtime {
        let lease_root = home.join("leases");
        let catalog = Arc::new(HarnessCatalog::builtin());
        let scope = RuntimeScope::fixture(home.to_path_buf());
        let db_path = scope.history_root.join("events.sqlite3");
        let ports: Ports =
            skill_studio_host::default_ports_with_history(lease_root, catalog, db_path);
        Runtime::new(&scope, ports).unwrap()
    }

    struct NeverFetch;
    impl UpstreamFetch for NeverFetch {
        fn fetch_skill_dir(&self, _: &str, _: &str, _: &str, _: &Path) -> Result<(), String> {
            panic!("fetch should not have been called");
        }
    }
    struct NeverLookup;
    impl CommitLookup for NeverLookup {
        fn latest_commit(
            &self,
            _: &str,
            _: &str,
            _: Option<&str>,
        ) -> Result<Option<(String, String)>, String> {
            panic!("lookup should not have been called");
        }
    }
    fn never_github() -> GithubTools {
        (Box::new(NeverFetch), Box::new(NeverLookup))
    }

    fn local_source(dir: &Path, name: &str) -> ParsedSkillSource {
        ParsedSkillSource {
            kind: ParsedSkillSourceKind::Local,
            repo: None,
            path: None,
            git_ref: None,
            skill_name: Some(name.to_string()),
            url: None,
            local_path: Some(dir.to_string_lossy().into_owned()),
        }
    }

    fn copy_request(source_dir: &Path, name: &str) -> AddSkillRequest {
        AddSkillRequest {
            source: local_source(source_dir, name),
            method: AddMethod::Copy,
            destination: SkillDestination::Universal,
            agents: vec![],
            disabled_harnesses: vec![],
            scope: InstallScope::Global,
            project_path: None,
            trial: false,
        }
    }

    fn github_source(repo: &str, name: &str) -> ParsedSkillSource {
        ParsedSkillSource {
            kind: ParsedSkillSourceKind::Github,
            repo: Some(repo.to_string()),
            path: None,
            git_ref: None,
            skill_name: Some(name.to_string()),
            url: None,
            local_path: None,
        }
    }

    /// The Skill Store's real request shape (`SkillStoreInstallFlow.tsx`):
    /// `method: "skills-sh"` against a GitHub source.
    fn skills_sh_request(repo: &str, name: &str) -> AddSkillRequest {
        AddSkillRequest {
            source: github_source(repo, name),
            method: AddMethod::SkillsSh,
            destination: SkillDestination::Universal,
            agents: vec![],
            disabled_harnesses: vec![],
            scope: InstallScope::Global,
            project_path: None,
            trial: false,
        }
    }

    /// "Promote to global"'s real method (`skill-location-actions.ts:227`)
    /// is `copy` against a `local` source, which never carries a repository
    /// identity to gate (`normalize_dotagents_source_identity` returns
    /// `None` for `Local` - promoting an already-installed local skill has
    /// nothing remote left to trust). This helper instead pairs `Copy` with
    /// a GitHub source, the only shape that exercises the same `trust_identity`
    /// threading `build_install_request` now does for `Copy`/`SkillsSh` (item
    /// 4): a Copy install of an *un-promoted* GitHub source still needs the
    /// same gate. See the module's test for the exact deviation this covers.
    fn copy_github_request(repo: &str, name: &str) -> AddSkillRequest {
        let mut request = skills_sh_request(repo, name);
        request.method = AddMethod::Copy;
        request
    }

    /// A `UpstreamFetch`/`CommitLookup` pair that actually succeeds, for a
    /// `Copy` request whose files are gathered before `ops::install`'s trust
    /// check runs (unlike `Dotagents`/`SkillsSh`, `Copy` always reads its
    /// source first - see `install_one`), so `NeverFetch`/`NeverLookup` would
    /// panic on it even when the write itself is later refused.
    struct StubFetch;
    impl UpstreamFetch for StubFetch {
        fn fetch_skill_dir(
            &self,
            _repo: &str,
            _path: &str,
            _commit: &str,
            into: &Path,
        ) -> Result<(), String> {
            super::super::test_support::write_skill(into, "visual-recap");
            Ok(())
        }
    }
    struct StubLookup;
    impl CommitLookup for StubLookup {
        fn latest_commit(
            &self,
            _repo: &str,
            _path: &str,
            _until: Option<&str>,
        ) -> Result<Option<(String, String)>, String> {
            Ok(Some((
                "deadbeef".to_string(),
                "2024-01-01T00:00:00Z".to_string(),
            )))
        }
    }
    fn stub_github() -> GithubTools {
        (Box::new(StubFetch), Box::new(StubLookup))
    }

    // Review item 5: two of the deleted `skill_add.rs`'s batch tests,
    // restored against `open_batch_snapshot`/`install_one` directly - the
    // new home for what `add_skills_with` used to drive.

    struct CountingFetch {
        downloads: std::sync::Mutex<usize>,
    }
    struct FakeSnapshot;
    impl RepoSnapshot for FakeSnapshot {
        fn copy_dir(&self, path: &str, into: &Path) -> Result<(), String> {
            let name = path.rsplit('/').next().unwrap_or(path);
            std::fs::create_dir_all(into).unwrap();
            std::fs::write(
                into.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: test\n---\nBody."),
            )
            .unwrap();
            Ok(())
        }
    }
    impl UpstreamFetch for CountingFetch {
        fn fetch_skill_dir(&self, _: &str, _: &str, _: &str, _: &Path) -> Result<(), String> {
            panic!("batch copy must use the snapshot");
        }
        fn open_repo(&self, _: &str, _: &str) -> Result<Option<Box<dyn RepoSnapshot>>, String> {
            *self.downloads.lock().unwrap() += 1;
            Ok(Some(Box::new(FakeSnapshot)))
        }
    }

    fn batch_request(entries: Vec<GithubSkillEntry>) -> AddSkillsRequest {
        let mut source = github_source("kentcdodds/kcd-skills", "");
        source.skill_name = None;
        source.git_ref = Some("main".to_string());
        source.path = Some("skills".to_string());
        AddSkillsRequest {
            source,
            skills: entries,
            method: AddMethod::Copy,
            destination: SkillDestination::Universal,
            agents: vec![],
            disabled_harnesses: vec![],
            scope: InstallScope::Global,
            project_path: None,
            trial: false,
        }
    }

    /// `copy_batch_downloads_the_repo_once_for_every_skill_or_names_the_extra_download`:
    /// adapted (review item 5) from the deleted `skill_add.rs` test of the
    /// same base name - `open_batch_snapshot` opens the shared repo once,
    /// then every entry's `install_one` reuses it instead of refetching.
    #[test]
    fn copy_batch_downloads_the_repo_once_for_every_skill_or_names_the_extra_download() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        super::super::skill_trust_policy::record_trusted_dotagents_source(
            home,
            "kentcdodds/kcd-skills",
        )
        .unwrap();
        let rt = test_runtime(home);
        let request = batch_request(vec![
            GithubSkillEntry {
                name: "visual-recap".to_string(),
                path: "skills/visual-recap".to_string(),
            },
            GithubSkillEntry {
                name: "other".to_string(),
                path: "skills/other".to_string(),
            },
            GithubSkillEntry {
                name: "third".to_string(),
                path: "skills/third".to_string(),
            },
        ]);
        let fetch = CountingFetch {
            downloads: std::sync::Mutex::new(0),
        };
        let snapshot = open_batch_snapshot(&request, &fetch, &StubLookup).unwrap();

        for entry in &request.skills {
            let entry_request = request_for_entry(&request, entry);
            let outcome = install_one(
                &rt,
                &entry_request,
                &fetch,
                &StubLookup,
                snapshot.as_deref(),
            )
            .unwrap();
            assert!(matches!(outcome, InstallAdapterOutcome::Result(_)));
        }

        assert_eq!(
            *fetch.downloads.lock().unwrap(),
            1,
            "one shared snapshot should serve the whole batch, not one per skill"
        );
        assert!(home.join(".agents/skills/visual-recap/SKILL.md").exists());
        assert!(home.join(".agents/skills/third/SKILL.md").exists());
    }

    /// `a_failed_skill_does_not_stop_the_rest_of_the_batch`: unchanged from
    /// the deleted `skill_add.rs` in spirit - an already-existing destination
    /// fails one entry while the next entry, called right after in the same
    /// loop a real caller (`skill_add_operation.rs`'s batch worker) would
    /// run, still installs.
    #[test]
    fn a_failed_skill_does_not_stop_the_rest_of_the_batch() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path();
        std::fs::create_dir_all(home.join(".agents/skills/other")).unwrap();
        super::super::skill_trust_policy::record_trusted_dotagents_source(
            home,
            "kentcdodds/kcd-skills",
        )
        .unwrap();
        let rt = test_runtime(home);
        let request = batch_request(vec![
            GithubSkillEntry {
                name: "other".to_string(),
                path: "skills/other".to_string(),
            },
            GithubSkillEntry {
                name: "visual-recap".to_string(),
                path: "skills/visual-recap".to_string(),
            },
        ]);
        let fetch = CountingFetch {
            downloads: std::sync::Mutex::new(0),
        };
        let snapshot = open_batch_snapshot(&request, &fetch, &StubLookup).unwrap();

        let Err(first) = install_one(
            &rt,
            &request_for_entry(&request, &request.skills[0]),
            &fetch,
            &StubLookup,
            snapshot.as_deref(),
        ) else {
            panic!("destination already exists, install_one should have failed");
        };
        assert!(first.contains("already exists"), "{first}");

        let second = install_one(
            &rt,
            &request_for_entry(&request, &request.skills[1]),
            &fetch,
            &StubLookup,
            snapshot.as_deref(),
        )
        .unwrap();
        assert!(matches!(second, InstallAdapterOutcome::Result(_)));
        assert!(home.join(".agents/skills/visual-recap/SKILL.md").exists());
    }

    /// `add_skill_runs_on_a_blocking_thread_not_the_ui_task_or_names_the_task_it_blocks`:
    /// same shape as `harness_first_run.rs`'s
    /// `detect_runs_the_probes_on_a_blocking_thread...` - the runtime-builder
    /// closure records the OS thread it ran on; under a `current_thread`
    /// Tokio runtime the test task is the only async worker, so a build that
    /// happened anywhere else must have gone through `spawn_blocking`. Fails
    /// (red-checked) if `add_skill_with_runtime` calls `build_runtime` or
    /// `ops::install` directly on the calling task instead of inside
    /// `spawn_blocking`.
    ///
    /// The `build_runtime` closure alone only proves where *it* ran - review
    /// item 11 - so this also arms `install_one`'s own
    /// `INSTALL_ONE_THREAD_PROBE` from inside that closure (which already
    /// runs on the `spawn_blocking` worker thread) and asserts `install_one`
    /// recorded a thread at all, and that it is the same one.
    #[tokio::test(flavor = "current_thread")]
    async fn add_skill_runs_on_a_blocking_thread_not_the_ui_task_or_names_the_task_it_blocks() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let source_dir = tmp.path().join("source");
        std::fs::create_dir_all(&home).unwrap();
        super::super::test_support::write_skill(&source_dir, "find-bugs");

        let rt = test_runtime(&home);
        let request = copy_request(&source_dir, "find-bugs");

        let test_task_thread = std::thread::current().id();
        let build_thread = Arc::new(std::sync::Mutex::new(None));
        let record_build_thread = Arc::clone(&build_thread);
        let install_thread = Arc::new(std::sync::Mutex::new(None));
        let record_install_thread = Arc::clone(&install_thread);

        let result = add_skill_with_runtime(
            move || {
                *record_build_thread.lock().unwrap() = Some(std::thread::current().id());
                set_install_one_thread_probe(record_install_thread);
                Ok(rt)
            },
            request,
            never_github(),
        )
        .await
        .unwrap();

        assert_eq!(result.name, "find-bugs");
        let recorded = build_thread
            .lock()
            .unwrap()
            .expect("the runtime builder never ran");
        assert_ne!(
            recorded, test_task_thread,
            "ops::install ran on the calling task ({test_task_thread:?}) instead of a \
             spawn_blocking pool thread"
        );
        let install_recorded = install_thread
            .lock()
            .unwrap()
            .expect("install_one never ran on the spawn_blocking worker thread");
        assert_eq!(
            install_recorded, recorded,
            "install_one ran on a different thread than build_runtime, so it did not run \
             inside the same spawn_blocking call"
        );
    }

    /// `store_install_of_an_untrusted_skills_sh_source_or_names_needs_trust_as_a_plain_error`:
    /// the Skill Store's real request (`SkillStoreInstallFlow.tsx:129`) is
    /// `method: "skills-sh"`, not `Dotagents` - fixed per review item 4/12, so
    /// this proves `build_install_request`'s explicit `trust_identity` for
    /// `SkillsSh` (not just core's own built-in `Dotagents` gate) reaches the
    /// prompt. The Store install flow has no operation event stream to read a
    /// structured `NeedsTrust` from (see the module doc), so it must see the
    /// same plain-error text `add_skill` returns for every other caller.
    #[tokio::test]
    async fn store_install_of_an_untrusted_skills_sh_source_or_names_needs_trust_as_a_plain_error()
    {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let rt = test_runtime(&home);
        let request = skills_sh_request("kentcdodds/kcd-skills", "visual-recap");

        let error = add_skill_with_runtime(move || Ok(rt), request, never_github())
            .await
            .unwrap_err();

        assert!(
            error.starts_with(UNTRUSTED_DOTAGENTS_SOURCE_PREFIX),
            "unexpected error: {error}"
        );
        assert!(error.contains("kentcdodds/kcd-skills"), "{error}");
    }

    /// `promote_to_global_of_an_untrusted_copy_source_or_names_needs_trust_as_a_plain_error`:
    /// "Promote to global"'s own method (`skill-location-actions.ts:227`) is
    /// `Copy`, fixed per review item 4/12 - proving `build_install_request`
    /// now threads an explicit `trust_identity` for `Copy` too, the same way
    /// it already does for `SkillsSh` above. `copy_github_request`'s own doc
    /// explains why this uses a GitHub source rather than promote's real
    /// `local` one: `Local` carries no repository identity to gate.
    #[tokio::test]
    async fn promote_to_global_of_an_untrusted_copy_source_or_names_needs_trust_as_a_plain_error() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let rt = test_runtime(&home);
        let request = copy_github_request("evil/repo", "promoted-skill");

        let error = add_skill_with_runtime(move || Ok(rt), request, stub_github())
            .await
            .unwrap_err();

        assert!(
            error.starts_with(UNTRUSTED_DOTAGENTS_SOURCE_PREFIX),
            "unexpected error: {error}"
        );
        assert!(error.contains("evil/repo"), "{error}");
    }

    /// `confirm_add_skill_trust_retries_the_same_request_and_installs_or_names_the_changed_field`:
    /// replaces the dropped `trusted_retry_uses_the_same_request` (see
    /// `skill_add_operation.rs`'s own doc on why that one was cut) - a
    /// `Copy` install can be retried end to end with only this adapter's own
    /// fakes, unlike a `Dotagents`/`SkillsSh` retry which needs a real `npx`.
    /// First call is refused with `NeedsTrust`; recording trust the same way
    /// `confirm_add_skill_trust` does, then retrying the identical request
    /// with `trust_confirmed` true, must install.
    #[tokio::test]
    async fn confirm_add_skill_trust_retries_the_same_request_and_installs_or_names_the_changed_field(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        let request = copy_github_request("kentcdodds/kcd-skills", "visual-recap");

        let rt = test_runtime(&home);
        let first_error = add_skill_with_runtime(move || Ok(rt), request.clone(), stub_github())
            .await
            .unwrap_err();
        assert!(
            first_error.starts_with(UNTRUSTED_DOTAGENTS_SOURCE_PREFIX),
            "unexpected error: {first_error}"
        );

        super::super::skill_trust_policy::record_trusted_dotagents_source(
            &home,
            "kentcdodds/kcd-skills",
        )
        .unwrap();

        let rt = test_runtime(&home);
        let result = add_skill_with_runtime(move || Ok(rt), request, stub_github())
            .await
            .unwrap_or_else(|e| panic!("retry after trust should have installed, or names the field it still refused: {e}"));
        assert_eq!(result.name, "visual-recap");
    }

    /// `add_skill_with_disabled_harnesses_ends_with_that_harness_switched_off_or_names_the_harness_still_enabled`
    /// (review item 3): decision 2 (`launch-3-5c.md`) runs
    /// `disabled_harnesses` as a follow-up after `ops::install`'s own write
    /// succeeds, directly through `ops::set_harness_enabled`. Installs Claude
    /// Code linked, then disabled, and checks the disk state
    /// `set_harness_enabled` itself mutates (the per-skill symlink under
    /// `.claude/skills`), not just that the call returned without an error.
    #[tokio::test]
    async fn add_skill_with_disabled_harnesses_ends_with_that_harness_switched_off_or_names_the_harness_still_enabled(
    ) {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let source_dir = tmp.path().join("source");
        std::fs::create_dir_all(&home).unwrap();
        super::super::test_support::write_skill(&source_dir, "find-bugs");

        let rt = test_runtime(&home);
        let mut request = copy_request(&source_dir, "find-bugs");
        request.agents = vec![AgentId::ClaudeCode];
        request.disabled_harnesses = vec![AgentId::ClaudeCode];

        let result = add_skill_with_runtime(move || Ok(rt), request, never_github())
            .await
            .unwrap();

        assert_eq!(
            result.warning, None,
            "disabling claude-code right after install should not have failed"
        );
        let link = home.join(".claude/skills/find-bugs");
        assert!(
            !link.exists(),
            "claude-code should end disabled, but {} still exists",
            link.display()
        );
    }

    /// `trial_recording_failure_surfaces_as_a_warning_and_keeps_the_install`
    /// (review item 5): re-added from the deleted `skill_add.rs`, adapted to
    /// this crate's write lease instead of a directory-shaped registry file
    /// - `ops::install` now writes that same registry document itself
    /// (`ops_install.rs`'s `<scope>/.agents/skill-studio.json`), so making
    /// it unwritable would fail the install, not just `finish_install`'s
    /// trial follow-up. Holding `write_lease.rs`'s own lease on `home`
    /// before the install starts hits only `record_trial`'s
    /// `try_acquire`, which is exactly the conflict this test needs:
    /// `finish_install` runs the trial follow-up after `ops::install`
    /// already wrote the skill, so a failure there must not undo the
    /// install, only warn.
    #[tokio::test]
    async fn trial_recording_failure_surfaces_as_a_warning_and_keeps_the_install() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let source_dir = tmp.path().join("source");
        std::fs::create_dir_all(&home).unwrap();
        super::super::test_support::write_skill(&source_dir, "find-bugs");

        let write_lease = super::super::write_lease::WriteLease::default();
        let _held = write_lease
            .try_acquire(&home)
            .expect("test should be the first writer on this fresh tempdir");

        let rt = test_runtime(&home);
        let mut request = copy_request(&source_dir, "find-bugs");
        request.trial = true;

        let result = add_skill_with_runtime(move || Ok(rt), request, never_github())
            .await
            .unwrap();

        assert!(home.join(".agents/skills/find-bugs").exists());
        let warning = result.warning.expect("expected a trial warning");
        assert!(warning.starts_with("trial:"), "{warning}");
    }

    /// `a_harness_that_cannot_be_disabled_becomes_a_warning_not_a_failed_install`
    /// (review item 5): re-added from the deleted `skill_add.rs`, adapted to
    /// `ops::set_harness_enabled`'s own refusal - unlike the legacy
    /// `skill_harness_disable.rs` dispatch this crate's `disable_harness`
    /// now goes through, `pi` has a native switch here (`set_pi_switch`), so
    /// `cursor` (no native per-skill switch at all, `ops.rs`'s `other =>`
    /// arm) is the one that still names the refusal. Asking to disable it is
    /// a `finish_install` follow-up failure, not an install failure.
    #[tokio::test]
    async fn a_harness_that_cannot_be_disabled_becomes_a_warning_not_a_failed_install() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let source_dir = tmp.path().join("source");
        std::fs::create_dir_all(&home).unwrap();
        super::super::test_support::write_skill(&source_dir, "find-bugs");

        let rt = test_runtime(&home);
        let mut request = copy_request(&source_dir, "find-bugs");
        request.disabled_harnesses = vec![AgentId::Cursor];

        let result = add_skill_with_runtime(move || Ok(rt), request, never_github())
            .await
            .unwrap();

        assert!(home.join(".agents/skills/find-bugs").exists());
        let warning = result.warning.expect("expected a disable warning");
        assert!(warning.contains("no native per-skill switch"), "{warning}");
    }

    // Review item 1: the deleted `skill_add.rs`'s four local-Copy-source
    // guard tests, restored against `gather_copy_files` directly (the guard
    // now lives in `guard_local_copy_source`, called from there before any
    // read) rather than the whole `add_skill` flow those originally ran
    // through - `gather_copy_files` is the one shared choke point both
    // `add_skill` and the background operation worker call through, so a
    // test here covers both.

    fn never_fetch_lookup() -> (NeverFetch, NeverLookup) {
        (NeverFetch, NeverLookup)
    }

    /// `local_copy_of_a_home_ancestor_is_refused_before_any_read_or_names_the_path`
    #[test]
    fn local_copy_of_a_home_ancestor_is_refused_before_any_read_or_names_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("SKILL.md"), "body").unwrap();
        let source = local_source(&home, "nested-copy");
        let destination = home.join(".agents/skills/nested-copy");
        let (fetch, lookup) = never_fetch_lookup();

        let error =
            gather_copy_files(&source, &home, &destination, &fetch, &lookup, None).unwrap_err();

        assert!(error.contains(&home.display().to_string()), "{error}");
        assert!(!home.join(".agents").exists());
    }

    /// `local_copy_of_a_project_ancestor_is_refused_before_any_read_or_names_the_path`
    #[test]
    fn local_copy_of_a_project_ancestor_is_refused_before_any_read_or_names_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(project.join("SKILL.md"), "body").unwrap();
        let source = local_source(&project, "nested-copy");
        let destination = project.join(".agents/skills/nested-copy");
        let (fetch, lookup) = never_fetch_lookup();

        let error =
            gather_copy_files(&source, &project, &destination, &fetch, &lookup, None).unwrap_err();

        assert!(error.contains(&project.display().to_string()), "{error}");
        assert!(!project.join(".agents").exists());
    }

    /// `local_copy_of_a_symlink_into_a_destination_ancestor_is_refused_or_names_the_resolved_path`
    #[cfg(unix)]
    #[test]
    fn local_copy_of_a_symlink_into_a_destination_ancestor_is_refused_or_names_the_resolved_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let source_link = tmp.path().join("source-link");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("SKILL.md"), "body").unwrap();
        std::os::unix::fs::symlink(&home, &source_link).unwrap();
        let source = local_source(&source_link, "nested-copy");
        let destination = home.join(".agents/skills/nested-copy");
        let (fetch, lookup) = never_fetch_lookup();

        let error =
            gather_copy_files(&source, &home, &destination, &fetch, &lookup, None).unwrap_err();

        assert!(error.contains(&home.display().to_string()), "{error}");
        assert!(!home.join(".agents").exists());
    }

    /// `local_copy_of_the_destination_itself_is_refused_without_a_write_or_names_the_path`
    #[test]
    fn local_copy_of_the_destination_itself_is_refused_without_a_write_or_names_the_path() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        let source = home.join(".agents/skills/existing");
        std::fs::create_dir_all(&source).unwrap();
        std::fs::write(source.join("SKILL.md"), "body").unwrap();
        let parsed_source = local_source(&source, "existing");
        let (fetch, lookup) = never_fetch_lookup();

        let error =
            gather_copy_files(&parsed_source, &home, &source, &fetch, &lookup, None).unwrap_err();

        assert!(error.contains(&source.display().to_string()), "{error}");
        assert_eq!(
            std::fs::read_to_string(source.join("SKILL.md")).unwrap(),
            "body"
        );
        assert_eq!(
            std::fs::read_dir(source.parent().unwrap()).unwrap().count(),
            1
        );
    }
}
