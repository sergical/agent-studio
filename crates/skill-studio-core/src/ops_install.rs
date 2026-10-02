//! `ops::install`: puts one skill on disk by [`InstallMethod::Copy`],
//! `Dotagents`, or `SkillsSh`, all through one write path.
//!
//! Every method takes the per-scope exclusive lease
//! ([`crate::ports::MutationSession::begin`]), which sweeps
//! [`journal_root`]'s `Copy` staging journal for a previous crash's stray
//! plan before returning - see that function's doc. `install` then records
//! an `install` journal row - the destination's backup (normally "absent",
//! per `docs/action-map/install.md` "Desired state") and the inverse that
//! undoes a completed install - before the first byte moves. `Copy` stages
//! its folder beside the destination and swaps it into place through
//! [`crate::fsops`]'s `stage`/`swap`, journaled through [`journal_root`]'s
//! `FsJournal`. `Dotagents`/`SkillsSh` call `npx ... add <source>` through
//! the process-spawner port and let the CLI write its own files; this op
//! does not stage-and-swap the CLI's own writes - redirecting it into a
//! temporary home to force that would fight the CLI's own layout
//! assumptions, per `docs/action-map/plan.md`'s Correction section.
//!
//! Trust: a `Dotagents` install's identity is always derived from `req.source`
//! itself (never the caller's own `trust_identity`, which a caller could
//! otherwise omit to skip the gate) - it must already be trusted (recorded
//! by an earlier confirmed install) or the call must itself set
//! `trust_confirmed`; otherwise nothing is written and
//! [`InstallOutcome::NeedsTrust`] is returned. `Copy` and `SkillsSh` still
//! honor an explicit `req.trust_identity` the same way, for a caller that
//! wants the gate on a source of its own. Ported from the desktop's
//! `skill_trust_policy.rs`, minus its `WriteLeaseGuard`-specific entry
//! points (this op always already holds the exclusive lease itself).
//!
//! Linking: `req.harnesses` is the `skills` CLI's `--agent` set, and
//! [`crate::install_targets`] turns it into the folders to write - the
//! shared copy, a relative link or a real copy per harness with its own
//! folder, and a reported skip. The native `Copy` method writes that plan
//! itself; `SkillsSh` passes the same set to the CLI and then makes any
//! link the CLI left out. `Dotagents` ignores the set except for its links.
//!
//! Preferences: `install` (when `save_as_preference`) and
//! [`install_preferences`] read and write `preferred_method`/
//! `preferred_harnesses` in `<scope>/.agents/skill-studio.json`, merged in
//! alongside whatever other top-level keys that document already carries -
//! `serde_json`'s `preserve_order` feature and `Map::shift_remove` (never
//! `Map::remove`) keep every other untouched key's own position stable
//! across a write - `write_version` itself always moves to the front, since
//! [`write_registry_document`] pulls it out of the map and back into
//! [`RawRegistryDocument`]'s own leading field.
//! The write itself goes through [`registry::write_registry_document_locked`]
//! via [`RawRegistryDocument`], the same lease-guarded, write-version-bumping
//! path the desktop's own `ForkRegistry` uses - `install` just doesn't know
//! that type's adapter-specific shape, so it wraps the raw JSON object
//! instead.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dto::{
    InstallFile, InstallHarnessResult, InstallMethod, InstallOutcome, InstallPreferences,
    InstallRequest,
};
use crate::error::{CoreError, ErrorCode};
use crate::events::{EventDraft, EventKind, EventStatus};
use crate::fsops::{self, Root};
use crate::identity::{AgentId, RootScope, SkillDestination, SkillName};
use crate::install_targets::{self, InstallPlan, StepAction};
use crate::journal::{FsJournal, PlanWriter};
use crate::ops::Operation;
use crate::ops_install_cli::{install_via_cli, validate_cli_project_path};
use crate::ports::{
    self, ExclusiveGuard, FileKind, MutationSession, OpContext, PlanStatus, Runtime, ScopeFs,
};
use crate::registry;

/// `<home>/.agents/skill-studio-journal` - the [`FsJournal`] root
/// [`crate::ports::MutationSession::begin`] reconciles for every op, and the
/// root `install_copy` stages/swaps `Copy`'s writes through. Distinct from
/// `.agents/skills` (the universal root itself) so a journal plan directory
/// never looks like an installed skill to `scan`.
///
/// Always rooted under the scope *home*, not a project: a `FsJournal` root
/// is only bookkeeping for the stage/swap primitive, not where its writes
/// land (that's `universal_root`, passed separately to `PlanWriter::begin`),
/// so one home-rooted journal lets `begin` sweep it on every call regardless
/// of which scope - home or a project - the op in progress targets.
pub(crate) fn journal_root(home: &Path) -> PathBuf {
    home.join(".agents").join("skill-studio-journal")
}

/// Brings up `<home>/.agents` and [`journal_root`] - always rooted at the
/// scope home (see `journal_root`'s own doc), so a project-scope install
/// must create this even though its own `targets.universal_root` never
/// reaches the home tree. `confine`'s own canonicalize needs its immediate
/// parent to already exist, so this brings `<home>/.agents` up first, one
/// level at a time, before confining the journal root itself.
pub(crate) fn ensure_journal_root(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    fs: &dyn ScopeFs,
) -> Result<(), CoreError> {
    let home_agents_dir = rt.scope.home.lexical.join(".agents");
    let scoped_home_agents_dir = ports::confine(&rt.scope, fs, &home_agents_dir)?;
    fs.create_dir_all(guard, &scoped_home_agents_dir)
        .map_err(|e| CoreError::io(&home_agents_dir, e))?;
    let root = journal_root(&rt.scope.home.lexical);
    let scoped_journal_root = ports::confine(&rt.scope, fs, &root)?;
    fs.create_dir_all(guard, &scoped_journal_root)
        .map_err(|e| CoreError::io(&root, e))
}

/// `<scope>/.agents/skill-studio.json` - the registry document `install`
/// and [`install_preferences`] read and write. Matches
/// `crate::ownership::skill_studio_json_path`, but only ever addressed
/// relative to the scope this op targets (home or one project), never the
/// scope home unconditionally the way ownership classification reads it.
pub(crate) fn registry_path(scope_root: &Path) -> PathBuf {
    scope_root.join(".agents").join("skill-studio.json")
}

pub(crate) fn scope_root(rt: &Runtime, scope: &RootScope) -> PathBuf {
    match scope {
        RootScope::Global => rt.scope.home.lexical.clone(),
        RootScope::Project(project) => project.0.clone(),
    }
}

/// A minimal [`registry::RegistryDocument`] so `install` can write
/// `<scope>/.agents/skill-studio.json` through the core's lease-guarded,
/// write-version-bumping writer without depending on the adapter's own
/// `ForkRegistry` type - this op only ever adds or reads a handful of
/// top-level keys (`copies`, `trusted_dotagents_sources`,
/// `preferred_method`, `preferred_harnesses`), and must not disturb any
/// other key a different build already wrote to the same file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RawRegistryDocument {
    #[serde(default)]
    write_version: u64,
    #[serde(flatten)]
    fields: serde_json::Map<String, serde_json::Value>,
}

impl registry::RegistryDocument for RawRegistryDocument {
    fn write_version(&self) -> u64 {
        self.write_version
    }

    fn set_write_version(&mut self, version: u64) {
        self.write_version = version;
    }
}

/// Reads `<scope_root>/.agents/skill-studio.json` as a JSON object, or an
/// empty one when it is missing - the same "downgrade to nothing recorded"
/// a missing registry gets elsewhere in this crate
/// (`crate::ownership::read_home_registry`). A file that exists but is
/// unreadable or not a JSON object is a different failure: the write-back
/// this seeds would otherwise wipe `added_folders`, `forks`, and the trust
/// list, so that case fails the install before any write instead (R7).
pub(crate) fn read_registry_document(
    fs: &dyn ScopeFs,
    scope_root: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, CoreError> {
    let path = registry_path(scope_root);
    let bytes = match fs.read_capped(&path, 8 * 1024 * 1024) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(serde_json::Map::new()),
        Err(e) => return Err(CoreError::io(path, e)),
    };
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        Ok(_) => {
            Err(CoreError::new(ErrorCode::Io, "skill-studio.json is not a JSON object").at(&path))
        }
        Err(e) => {
            Err(CoreError::new(ErrorCode::Io, format!("corrupt registry file: {e}")).at(&path))
        }
    }
}

/// Writes `document` back to `<scope_root>/.agents/skill-studio.json`,
/// through [`registry::write_registry_document_locked`] under the caller's
/// already-held exclusive lease - `write_version` is bumped there, not by
/// this op, and every key besides the handful `install` itself touches
/// round-trips untouched.
pub(crate) fn write_registry_document(
    guard: &ExclusiveGuard,
    fs: &dyn ScopeFs,
    scope_root: &Path,
    document: serde_json::Map<String, serde_json::Value>,
) -> Result<(), CoreError> {
    let mut document = document;
    let write_version = document
        .shift_remove("write_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let mut wrapped = RawRegistryDocument {
        write_version,
        fields: document,
    };
    let path = registry_path(scope_root);
    registry::write_registry_document_locked(guard, fs, scope_root, &path, &mut wrapped)
}

/// Normalizes a `trust_identity` for lookup/storage: trims whitespace, drops
/// a leading `git:` protocol tag (a `Dotagents` git-URL source's
/// `req.source` carries one - `skill_add.rs`'s `format!("git:{url}")` - but
/// the desktop's own stored identity never does, per
/// `normalize_git_url_identity`), drops a trailing `/` and `.git`, lowercases.
/// Mirrors the desktop's `skill_trust_policy::normalize_dotagents_source_identity`/
/// `normalize_git_url_identity` byte-for-byte, minus the multi-line/empty
/// rejection (an empty identity is never gated by this op) - a source
/// already trusted through the desktop must not re-prompt here (R8).
fn normalize_identity(identity: &str) -> String {
    identity
        .trim()
        .strip_prefix("git:")
        .unwrap_or_else(|| identity.trim())
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_ascii_lowercase()
}

/// True when `identity` is already recorded as trusted in the registry
/// document's `trusted_dotagents_sources` array - the same key the
/// desktop's `ForkRegistry.trusted_dotagents_sources` reads and writes.
fn is_trusted(document: &serde_json::Map<String, serde_json::Value>, identity: &str) -> bool {
    document
        .get("trusted_dotagents_sources")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(identity)))
}

/// Adds `identity` to the registry document's `trusted_dotagents_sources`
/// array, deduplicated, preserving every other key already in `document`.
fn record_trusted(document: &mut serde_json::Map<String, serde_json::Value>, identity: &str) {
    let mut list: Vec<String> = document
        .get("trusted_dotagents_sources")
        .and_then(serde_json::Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if !list.iter().any(|s| s == identity) {
        list.push(identity.to_string());
    }
    document.insert(
        "trusted_dotagents_sources".to_string(),
        serde_json::Value::Array(list.into_iter().map(serde_json::Value::String).collect()),
    );
}

pub(crate) fn method_wire_name(method: InstallMethod) -> &'static str {
    match method {
        InstallMethod::Copy => "copy",
        InstallMethod::Dotagents => "dotagents",
        InstallMethod::SkillsSh => "skills_sh",
    }
}

fn method_from_wire_name(name: &str) -> Option<InstallMethod> {
    match name {
        "copy" => Some(InstallMethod::Copy),
        "dotagents" => Some(InstallMethod::Dotagents),
        "skills_sh" => Some(InstallMethod::SkillsSh),
        _ => None,
    }
}

/// Builds the same `dep:v1/{scope}/{slot}/{destination}/{name}/{project}/
/// {lexical-entry}` id the desktop's `skill_deployment::deployment_id` does,
/// via `ops::deployment_id`, for one real folder a `Copy` install wrote:
/// the shared copy (`Universal`, slot `universal`) or a harness's own copy
/// (`PerHarness`, slot `ops::harness_slot`).
pub(crate) fn copy_deployment_id(
    scope: &RootScope,
    skill: &SkillName,
    path: &Path,
    destination: SkillDestination,
    slot: &str,
) -> String {
    let scope_label = crate::ops::scope_label(scope);
    let project_path = match scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.to_string_lossy()),
    };
    crate::ops::deployment_id(
        &skill.0,
        scope_label,
        destination,
        slot,
        project_path.as_deref(),
        path,
    )
    .as_str()
    .to_string()
}

/// Reads `preferred_method`/`preferred_harnesses` from the scope's
/// registry document. Falls back to an environment default - `SkillsSh`
/// when `npx` resolves on `PATH` (through [`crate::ports::Ports::tools`]),
/// `Copy` otherwise - and no pre-selected harnesses, when nothing has been
/// saved yet. Ported from the desktop's `add_method_defaults.rs`, reduced
/// to the one fact `install`'s method default actually needs: whether the
/// CLIs this build's `Dotagents`/`SkillsSh` methods shell out to
/// (`npx ...`) can run at all.
pub fn install_preferences(
    rt: &Runtime,
    ctx: &OpContext,
    scope: &RootScope,
) -> Result<InstallPreferences, CoreError> {
    rt.run(Operation::InstallPreferences, ctx, || {
        install_preferences_body(rt, scope)
    })
}

fn install_preferences_body(
    rt: &Runtime,
    scope: &RootScope,
) -> Result<InstallPreferences, CoreError> {
    let root = scope_root(rt, scope);
    let fs = rt.ports.fs.as_ref();
    let document = read_registry_document(fs, &root)?;
    let method = document
        .get("preferred_method")
        .and_then(serde_json::Value::as_str)
        .and_then(method_from_wire_name);
    let harnesses = document
        .get("preferred_harnesses")
        .and_then(|v| serde_json::from_value::<Vec<AgentId>>(v.clone()).ok());
    if let (Some(method), Some(harnesses)) = (method, harnesses) {
        return Ok(InstallPreferences {
            method,
            harnesses,
            saved: true,
        });
    }
    let npx_on_path = rt
        .ports
        .tools
        .as_ref()
        .is_some_and(|tools| tools.find_binary("npx").is_some());
    Ok(InstallPreferences {
        method: if npx_on_path {
            InstallMethod::SkillsSh
        } else {
            InstallMethod::Copy
        },
        harnesses: Vec::new(),
        saved: false,
    })
}

/// Installs one skill by `req.method` for `req.harnesses`, under the
/// exclusive lease over `req.scope`'s root - see the module doc for the
/// write shape each method takes, and [`crate::install_targets`] for where
/// each harness's copy or link goes.
pub fn install(
    rt: &Runtime,
    ctx: &OpContext,
    req: &InstallRequest,
) -> Result<InstallOutcome, CoreError> {
    rt.run(Operation::Install, ctx, || install_body(rt, ctx, req))
}

fn install_body(
    rt: &Runtime,
    ctx: &OpContext,
    req: &InstallRequest,
) -> Result<InstallOutcome, CoreError> {
    ctx.checkpoint()?;
    install_targets::validate_harnesses(&req.harnesses)?;
    // Before anything below creates so much as a directory: `ensure_dir_all`
    // (further down, via `install_and_link`) `mkdir -p`s
    // `<project>/.agents/skills`, which would silently create a missing
    // project directory as a side effect and mask this exact fault.
    validate_cli_project_path(rt, req)?;
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let session = MutationSession::begin_for(rt, ctx, std::slice::from_ref(&req.skill));
    ctx.take_timing();
    let mut session = session?;
    let begin_step = crate::timing::step(clock, "begin_session", step_start);

    let fs = rt.ports.fs.as_ref();
    let root = scope_root(rt, &req.scope);
    let home_root = rt.scope.home.lexical.clone();
    let mut document = read_registry_document(fs, &root)?;
    // R1: `copies` and the trust list are always the home registry's, never
    // a project's own `<project>/.agents/skill-studio.json` - the desktop's
    // ownership classifier (`ownership.rs::read_home_registry`) only ever
    // opens the home file, for either scope. `None` when this install's own
    // scope root already *is* the home root (Global), so `document` alone
    // stays the single copy written back - a second read+write of the same
    // file would race its own write-version bump.
    let mut home_document = if root == home_root {
        None
    } else {
        Some(read_registry_document(fs, &home_root)?)
    };

    // Trust: a `Dotagents` install's identity always comes from `req.source`
    // itself, never the caller's own `trust_identity` - a caller cannot skip
    // the gate by simply not setting it. `Copy`/`SkillsSh` still honor an
    // explicit `trust_identity`, for a caller that wants the same gate on a
    // source of its own.
    let trust_identity = match req.method {
        InstallMethod::Dotagents => req.source.as_deref().map(normalize_identity),
        InstallMethod::Copy | InstallMethod::SkillsSh => {
            req.trust_identity.as_deref().map(normalize_identity)
        }
    };
    if let Some(identity) = &trust_identity {
        let home_doc = home_document.as_mut().unwrap_or(&mut document);
        if !req.trust_confirmed && !is_trusted(home_doc, identity) {
            return Ok(InstallOutcome::NeedsTrust {
                identity: identity.clone(),
            });
        }
        if req.trust_confirmed {
            record_trusted(home_doc, identity);
        }
    }

    let per_harness = req.destination == SkillDestination::PerHarness;
    if per_harness && req.method != InstallMethod::Copy {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "a per-agent destination needs the Copy method: the CLIs write the shared folder",
        ));
    }
    let harnesses = install_targets::requested_harnesses(&req.harnesses);
    let mode = if per_harness {
        crate::dto::InstallLinkMode::Copy
    } else {
        install_targets::effective_link_mode(req.method, &harnesses, &req.scope, req.link_mode)?
    };
    let mut plan = install_targets::plan_install(
        fs,
        &root,
        &req.scope,
        &rt.scope,
        &req.skill,
        &harnesses,
        mode,
        req.destination,
    )?;
    // A skipped harness never reaches the skills CLI, so its folder must not
    // count toward the link mode: the CLI copies when it sees one distinct
    // folder. The native `Copy` method keeps the requested set's mode.
    let served_mode = if req.method == InstallMethod::SkillsSh {
        install_targets::effective_link_mode(
            req.method,
            &plan.served_harnesses(),
            &req.scope,
            req.link_mode,
        )?
    } else {
        mode
    };
    if served_mode != mode {
        plan = install_targets::plan_install(
            fs,
            &root,
            &req.scope,
            &rt.scope,
            &req.skill,
            &harnesses,
            served_mode,
            req.destination,
        )?;
    }
    let mode = plan.mode;
    let destination = plan
        .primary_path()
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                "the chosen agents leave nothing to write",
            )
        })?
        .to_path_buf();
    let written_paths = plan.written_paths();
    if let Some(existing) = written_paths
        .iter()
        .find(|p| fs.symlink_metadata(p).is_ok())
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "a copy already exists at this destination; install does not overwrite one",
        )
        .at(existing));
    }
    for path in &written_paths {
        confine_planned(rt, fs, path)?;
    }

    let step_start = clock.monotonic();
    let id = rt.ports.ids.next_event_id();
    // The row goes down before the first byte moves (F7): its backup is
    // whatever currently sits at every path the plan writes - normally
    // nothing, which `backup_paths` records as "absent", itself the
    // pre-state a later restore compares against - and its inverse
    // describes undoing a completed install.
    let manifest = session
        .store
        .backup_paths(&session.guard, &id, &written_paths)?;
    // R6: the shared `restore_backup` shape every other write-then-record op
    // uses - not a one-off `remove_install` shape nothing parses (`events.rs`
    // only recognizes `restore_backup`/`recreate_symlink`/`remove_symlink`).
    // `pre` is always `None` (absent): every written path was checked above
    // to not exist yet, so `backup_paths` already recorded it as "absent" in
    // the manifest this inverse's `backup_dir` points at. `post` and the
    // other written paths are patched in after the write, when their
    // fingerprints exist; a crash before that leaves the row `Pending`,
    // which undo refuses anyway.
    let inverse = crate::events::restore_backup_inverse(&destination, None, None);
    let draft = EventDraft {
        kind: EventKind::Install,
        skill: req.skill.clone(),
        harness: None,
        scope: Some(crate::ops::scope_label(&req.scope).to_string()),
        project_path: match &req.scope {
            RootScope::Global => None,
            RootScope::Project(p) => Some(p.0.clone()),
        },
        payload: serde_json::json!({
            "method": method_wire_name(req.method),
            "destination": destination,
            "source": req.source,
            "harnesses": harnesses,
            "link_mode": mode,
        }),
        inverse: Some(inverse),
        backup_dir: Some(manifest.backup_dir.clone()),
    };
    session.store.record(&session.guard, &id, &draft)?;

    // F9: `ensure_dir_all`, the writes, the links, and the registry write
    // all share this one fallible step, so any of their failures - not just
    // the first write's - marks the row `Failed` instead of leaving it
    // `Pending`.
    let documents = RegistryDocuments {
        scope: document,
        home: home_document,
    };
    let targets = InstallTargets {
        root: &root,
        destination: &destination,
        plan: &plan,
    };
    match install_and_link(rt, ctx, &mut session, fs, req, &targets, documents) {
        Err(e) => {
            remove_written(rt, &session, fs, &written_paths);
            let _ = session
                .store
                .finish(&session.guard, &id, EventStatus::Failed, None);
            Err(e)
        }
        Ok((harness_results, registry_undo)) => {
            let mut inverse_patch = written_inverse_patch(fs, &destination, &written_paths);
            inverse_patch["registry_undo"] = registry_undo.to_json();
            let _ = session
                .store
                .patch_inverse(&session.guard, &id, inverse_patch);
            session
                .store
                .finish(&session.guard, &id, EventStatus::Done, None)?;
            session.finish(rt, ctx);
            let write_step = crate::timing::step(clock, "write_and_link", step_start);
            ctx.record_timing(crate::timing::op_timing(
                clock,
                "install",
                op_start,
                vec![begin_step, write_step],
            ));
            Ok(InstallOutcome::Installed {
                event_id: id,
                skill: req.skill.clone(),
                deployment_path: destination,
                linked_harnesses: linked_harnesses(&harness_results),
                harness_results,
            })
        }
    }
}

/// [`ports::confine`] for a path whose folders `install` has yet to make:
/// the nearest existing ancestor of the parent stands in for the parent, so
/// a harness folder under a link out of the scope is refused before the
/// journal row and the first write.
fn confine_planned(rt: &Runtime, fs: &dyn ScopeFs, path: &Path) -> Result<(), CoreError> {
    let parent = path.parent().unwrap_or(path);
    let resolved_parent = crate::ops::codex_path_form(fs, parent);
    let lexical_ok = path.is_absolute()
        && !path
            .components()
            .any(|c| c == std::path::Component::ParentDir);
    if lexical_ok && rt.scope.contains(path) && rt.scope.contains(&resolved_parent) {
        return Ok(());
    }
    Err(CoreError::new(ErrorCode::InvalidRequest, "path lies outside the scope").at(path))
}

/// Best-effort cleanup after a failed install. Every path in `written` was
/// absent before the install began, so whatever is there now is this
/// install's own (or its CLI's). Links go before the shared folder they
/// point at.
fn remove_written(rt: &Runtime, session: &MutationSession, fs: &dyn ScopeFs, written: &[PathBuf]) {
    for path in written.iter().rev() {
        let Ok(facts) = fs.symlink_metadata(path) else {
            continue;
        };
        if facts.kind == FileKind::Dir {
            crate::ops_remove::remove_tree_best_effort(fs, path);
        } else if let Ok(scoped) = ports::confine(&rt.scope, fs, path) {
            let _ = fs.remove_file(&session.guard, &scoped);
        }
    }
}

/// The inverse fields only a finished install knows: the primary folder's
/// fingerprint, and every other written link or folder with its own, for
/// undo to drift-check and remove.
fn written_inverse_patch(
    fs: &dyn ScopeFs,
    destination: &Path,
    written: &[PathBuf],
) -> serde_json::Value {
    let fingerprint = |path: &Path| crate::events::fingerprint_path(fs, path).ok().flatten();
    let others: Vec<(PathBuf, crate::identity::Fingerprint)> = written
        .iter()
        .filter(|p| p.as_path() != destination)
        .filter_map(|p| fingerprint(p).map(|f| (p.clone(), f)))
        .collect();
    let post =
        fingerprint(destination).map_or_else(|| "absent".to_string(), |f| f.bare_hex().to_string());
    crate::events::with_remove_copies(serde_json::json!({ "post_fingerprint": post }), &others)
}

/// Harnesses with their own folder that now see the skill through a link:
/// a new per-skill link, or a whole-folder link into the shared folder.
fn linked_harnesses(results: &[InstallHarnessResult]) -> Vec<AgentId> {
    results
        .iter()
        .filter_map(|r| match r {
            InstallHarnessResult::Linked { harness, .. } => Some(harness.clone()),
            InstallHarnessResult::ReadsShared { harness, .. }
                if install_targets::has_own_folder(harness) =>
            {
                Some(harness.clone())
            }
            _ => None,
        })
        .collect()
}

/// What `install_and_link` writes, bundled so it stays under clippy's
/// argument-count lint.
struct InstallTargets<'a> {
    root: &'a Path,
    /// [`InstallPlan::primary_path`].
    destination: &'a Path,
    plan: &'a InstallPlan,
}

/// `install`'s two registry documents, bundled so `install_and_link` stays
/// under clippy's argument-count lint. `scope` is `req.scope`'s own
/// registry (preferences); `home`, when `Some`, is the scope home's, for a
/// project install whose scope root differs from home - see `install`'s own
/// doc on why the two can diverge (R1).
struct RegistryDocuments {
    scope: serde_json::Map<String, serde_json::Value>,
    home: Option<serde_json::Map<String, serde_json::Value>>,
}

/// The write-and-link step every `install` call shares, once its journal
/// row is already recorded: writes `req.method`'s bytes, makes each
/// harness's link or copy, and writes the registry document(s) back - any
/// failure here bubbles up so `install` can mark the row `Failed` (F9).
fn install_and_link(
    rt: &Runtime,
    ctx: &OpContext,
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    req: &InstallRequest,
    targets: &InstallTargets,
    documents: RegistryDocuments,
) -> Result<(Vec<InstallHarnessResult>, RegistryUndo), CoreError> {
    let RegistryDocuments {
        scope: mut document,
        home: mut home_document,
    } = documents;
    // Compared against after every mutation below, so an install that never
    // touches `document` itself (no `save_as_preference`, and either not a
    // `Copy` or a `Copy` whose `copies` entry lands in `home_document`
    // instead) skips the scope write entirely, rather than creating
    // `<scope>/.agents/skill-studio.json` holding nothing but a bumped
    // `write_version`.
    let original_document = document.clone();
    let plan = targets.plan;
    let native = req.method == InstallMethod::Copy;
    if plan.shared.is_some() {
        crate::ops::ensure_dir_all(rt, session, fs, &plan.universal_root)?;
    }

    if !native {
        install_via_cli(rt, ctx, req, &plan.served_harnesses(), targets.destination)?;
    } else if plan.shared.is_some() {
        install_copy(
            rt,
            &session.guard,
            &plan.universal_root,
            &req.skill,
            &req.files,
        )?;
    }

    // Every real folder this install wrote, with the harness whose own
    // folder holds it (`None` for the shared copy) - the native method
    // records each in `copies`.
    let mut real_folders: Vec<(PathBuf, Option<AgentId>)> =
        plan.shared.iter().map(|p| (p.clone(), None)).collect();
    let mut results = Vec::with_capacity(plan.steps.len());
    for step in &plan.steps {
        let harness = step.harness.clone();
        let result = match &step.action {
            StepAction::ReadsShared { path } => InstallHarnessResult::ReadsShared {
                harness,
                path: path.clone(),
            },
            StepAction::Skip { reason } => InstallHarnessResult::Skipped {
                harness,
                reason: reason.clone(),
            },
            StepAction::Copy { dir, path } => {
                if native {
                    crate::ops::ensure_dir_all(rt, session, fs, dir)?;
                    install_copy(rt, &session.guard, dir, &req.skill, &req.files)?;
                } else if fs.symlink_metadata(path).is_err() {
                    return Err(CoreError::new(
                        ErrorCode::Io,
                        "the CLI did not create the expected agent copy",
                    )
                    .at(path));
                }
                real_folders.push((path.clone(), Some(harness.clone())));
                InstallHarnessResult::Copied {
                    harness,
                    path: path.clone(),
                    link_failed: false,
                }
            }
            StepAction::Link { dir, link } => {
                let shared = plan.shared.as_deref().ok_or_else(|| {
                    CoreError::new(ErrorCode::Io, "a link step needs the shared copy")
                })?;
                match link_or_copy(rt, session, fs, req, shared, dir, link)? {
                    LinkResult::Linked => InstallHarnessResult::Linked {
                        harness,
                        path: link.clone(),
                    },
                    LinkResult::Copied => {
                        real_folders.push((link.clone(), Some(harness.clone())));
                        InstallHarnessResult::Copied {
                            harness,
                            path: link.clone(),
                            link_failed: true,
                        }
                    }
                }
            }
        };
        results.push(result);
    }

    let scope_root = targets.root.to_path_buf();
    let mut undo = RegistryUndo::default();
    if req.save_as_preference {
        for (key, value) in [
            (
                "preferred_method",
                serde_json::Value::String(method_wire_name(req.method).to_string()),
            ),
            (
                "preferred_harnesses",
                serde_json::to_value(&req.harnesses)
                    .unwrap_or(serde_json::Value::Array(Vec::new())),
            ),
        ] {
            undo.push(&scope_root, key, None, document.get(key));
            document.insert(key.to_string(), value);
        }
    }
    if native {
        let home_root = rt.scope.home.lexical.clone();
        let home_doc = home_document.as_mut().unwrap_or(&mut document);
        for (path, harness) in &real_folders {
            let (id, replaced) = record_copy(fs, ctx, home_doc, req, path, harness.as_ref())?;
            undo.push(&home_root, "copies", Some(&id), replaced.as_ref());
        }
    }
    if document != original_document {
        write_registry_document(&session.guard, fs, targets.root, document)?;
    }
    if let Some(home_doc) = home_document {
        // A project-scope install never otherwise touches the home root, but
        // `write_registry_document_locked` already creates
        // `<home>/.agents` itself before writing the file into it, so this
        // needs no `ensure_dir_all` call of its own.
        write_registry_document(&session.guard, fs, &rt.scope.home.lexical, home_doc)?;
    }

    Ok((results, undo))
}

/// One registry key an install wrote, with the value it held before: the
/// `restore_backup` inverse's `registry_undo` list, which `restore_event`
/// replays through [`restore_registry`] so undo puts back the preferences
/// and drops the `copies` entries the install added.
#[derive(Debug, Default)]
pub(crate) struct RegistryUndo(Vec<serde_json::Value>);

impl RegistryUndo {
    /// `id` names one entry inside the `key` map (`copies`); `None` means the
    /// top-level `key` itself. `previous` is `None` when nothing was there.
    fn push(
        &mut self,
        root: &Path,
        key: &str,
        id: Option<&str>,
        previous: Option<&serde_json::Value>,
    ) {
        self.0.push(serde_json::json!({
            "root": root,
            "key": key,
            "id": id,
            "previous": previous,
        }));
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Array(self.0.clone())
    }
}

/// One `registry_undo` entry for a row an update or remove changed. Unlike
/// [`RegistryUndo::push`]'s install entries, it also records `expected`, the
/// row as that event left it (`None`: absent). [`check_registry_drift`]
/// compares it with the live row before undo, so a row someone else changed
/// since is never overwritten without `force`.
pub(crate) fn guarded_registry_undo(
    root: &Path,
    key: &str,
    id: &str,
    previous: Option<&serde_json::Value>,
    expected: Option<&serde_json::Value>,
) -> serde_json::Value {
    serde_json::json!({
        "root": root,
        "key": key,
        "id": id,
        "previous": previous,
        "expected": expected,
    })
}

/// Refuses an undo whose `registry_undo` entries carry an `expected` row that
/// no longer matches the live registry. Entries without `expected` (installs,
/// older events) are never checked.
pub(crate) fn check_registry_drift(
    fs: &dyn ScopeFs,
    inverse: &serde_json::Value,
) -> Result<(), CoreError> {
    let Some(edits) = inverse
        .get("registry_undo")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(());
    };
    for edit in edits {
        let (Some(expected), Some(root), Some(key), Some(id)) = (
            edit.get("expected"),
            edit.get("root").and_then(serde_json::Value::as_str),
            edit.get("key").and_then(serde_json::Value::as_str),
            edit.get("id").and_then(serde_json::Value::as_str),
        ) else {
            continue;
        };
        let document = read_registry_document(fs, Path::new(root))?;
        let live = document
            .get(key)
            .and_then(|map| map.get(id))
            .unwrap_or(&serde_json::Value::Null);
        if live != expected {
            return Err(CoreError::new(
                ErrorCode::DriftConflict,
                format!(
                    "the {key} registry row {id} changed since this event; pass force to restore anyway"
                ),
            )
            .at(registry_path(Path::new(root))));
        }
    }
    Ok(())
}

/// Replays an inverse's `registry_undo` list: each key goes back to its
/// recorded value, or is removed when it held none. An inverse without the
/// list (an older event) changes nothing. Only the recorded keys are
/// touched, so registry edits made since the install stay.
pub(crate) fn restore_registry(
    guard: &ExclusiveGuard,
    fs: &dyn ScopeFs,
    inverse: &serde_json::Value,
) -> Result<(), CoreError> {
    let Some(edits) = inverse
        .get("registry_undo")
        .and_then(serde_json::Value::as_array)
    else {
        return Ok(());
    };
    let root_of = |edit: &serde_json::Value| {
        edit.get("root")
            .and_then(serde_json::Value::as_str)
            .map(PathBuf::from)
    };
    let mut roots: Vec<PathBuf> = Vec::new();
    for root in edits.iter().filter_map(root_of) {
        if !roots.contains(&root) {
            roots.push(root);
        }
    }
    for root in roots {
        let mut document = read_registry_document(fs, &root)?;
        let original = document.clone();
        for edit in edits.iter().filter(|e| root_of(e).as_ref() == Some(&root)) {
            let Some(key) = edit.get("key").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let previous = edit.get("previous").filter(|v| !v.is_null()).cloned();
            match edit.get("id").and_then(serde_json::Value::as_str) {
                None => match previous {
                    Some(value) => {
                        document.insert(key.to_string(), value);
                    }
                    None => {
                        document.shift_remove(key);
                    }
                },
                Some(id) => {
                    if let Some(serde_json::Value::Object(map)) = document.get_mut(key) {
                        match previous {
                            Some(value) => {
                                map.insert(id.to_string(), value);
                            }
                            None => {
                                map.shift_remove(id);
                            }
                        }
                    }
                }
            }
        }
        if document != original {
            write_registry_document(guard, fs, &root, document)?;
        }
    }
    Ok(())
}

/// Adds the `copies` entry for one real folder the native method wrote.
/// R1: keyed by the deployment id, not the skill name - every consumer
/// (`ops.rs::classify_owner`'s `home_registry.copies.get(cx.id)`, the
/// desktop's `commands.rs`/`skill_harness_disable.rs`) looks this map up by
/// id, never by name. R2: a non-empty `content_hash` - the desktop's
/// `CopyDeploymentRecord` doc says empty is legacy-only, and destructive
/// mutations refuse it.
fn record_copy(
    fs: &dyn ScopeFs,
    ctx: &OpContext,
    home_doc: &mut serde_json::Map<String, serde_json::Value>,
    req: &InstallRequest,
    path: &Path,
    harness: Option<&AgentId>,
) -> Result<(String, Option<serde_json::Value>), CoreError> {
    let (destination, slot) = match harness {
        None => (SkillDestination::Universal, "universal".to_string()),
        Some(harness) => (
            SkillDestination::PerHarness,
            crate::ops::harness_slot(harness),
        ),
    };
    let deployment_id = copy_deployment_id(&req.scope, &req.skill, path, destination, &slot);
    let content_hash = crate::ops::skill_content_hash(fs, ctx, path)?;
    let copies = home_doc
        .entry("copies".to_string())
        .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
    let mut replaced = None;
    if let serde_json::Value::Object(copies) = copies {
        replaced = copies.insert(
            deployment_id.clone(),
            serde_json::json!({
                "deployment_id": deployment_id,
                "name": req.skill.0,
                "path": path,
                "scope": crate::ops::scope_label(&req.scope),
                "destination": destination,
                "slot": slot,
                "project_path": match &req.scope {
                    RootScope::Global => None,
                    RootScope::Project(p) => Some(p.0.clone()),
                },
                "content_hash": content_hash,
                "disabled": false,
            }),
        );
    }
    Ok((deployment_id, replaced))
}

/// `Copy`: stages `files` under [`journal_root`]'s [`FsJournal`], then
/// swaps the staged folder into `<skills_dir>/<skill>`. The journal's
/// own crash from a previous install is already swept by this call's
/// `MutationSession::begin`, so this only opens it, never reconciles it a
/// second time.
fn install_copy(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    skills_dir: &Path,
    skill: &SkillName,
    files: &[InstallFile],
) -> Result<(), CoreError> {
    let fs = rt.ports.fs.clone();
    // R1 fallout: this journal is always rooted under the scope home (see
    // the doc on `journal_root`'s only call site in `MutationSession::begin`),
    // never under the op's own target root - so for a project-scope install,
    // `<home>/.agents` was never brought up by the caller's own
    // `ensure_dir_all`, which only reaches the *project's* folders.
    ensure_journal_root(rt, guard, fs.as_ref())?;
    let journal_root = journal_root(&rt.scope.home.lexical);
    let journal = FsJournal::new(journal_root, fs.clone());

    let root = Root::open(fs.as_ref(), skills_dir.to_path_buf())
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(skills_dir))?;
    let plan_id = crate::identity::PlanId(rt.ports.ids.next_event_id().0);
    let plan = PlanWriter::begin(
        &journal,
        guard,
        plan_id,
        rt.ports.clock.now(),
        format!("install {}", skill.0),
        skills_dir.to_path_buf(),
        Vec::new(),
    )
    .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?;

    let contents: Vec<fsops::StageFile> = files
        .iter()
        .map(|f| fsops::StageFile {
            relative: f.relative_path.clone(),
            bytes: f.contents.clone(),
            mode: f.mode,
        })
        .collect();
    let staged = fsops::stage_files(&root, &plan, &contents)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(skills_dir))?;
    let final_name = Path::new(&skill.0);
    let quarantine_dir = Path::new(".skill-studio-install-quarantine");
    fsops::swap(&root, &plan, final_name, &staged, quarantine_dir)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(skills_dir))?;
    plan.finish(PlanStatus::Done)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?;
    Ok(())
}

enum LinkResult {
    Linked,
    Copied,
}

/// Makes the relative link `link` (inside the harness folder `dir`) to the
/// shared copy. A `Dotagents`/`SkillsSh` CLI may already have made it, or
/// copied a folder there after its own symlink failed; both count as done
/// (R3, mirrors the desktop's `skill_add::maybe_claude_code_symlink`). When
/// the symlink itself fails, the native method copies the folder instead,
/// the same fallback as the `skills` CLI.
fn link_or_copy(
    rt: &Runtime,
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    req: &InstallRequest,
    shared: &Path,
    dir: &Path,
    link: &Path,
) -> Result<LinkResult, CoreError> {
    if let Ok(existing) = fs.symlink_metadata(link) {
        return Ok(if existing.kind == FileKind::Symlink {
            LinkResult::Linked
        } else {
            LinkResult::Copied
        });
    }
    crate::ops::ensure_dir_all(rt, session, fs, dir)?;
    let scoped_target = ports::confine(&rt.scope, fs, shared)?;
    let scoped_link = ports::confine(&rt.scope, fs, link)?;
    let from = fs.canonicalize(dir).map_err(|e| CoreError::io(dir, e))?;
    let to = fs
        .canonicalize(shared)
        .map_err(|e| CoreError::io(shared, e))?;
    let relative = install_targets::relative_path(&from, &to);
    match fs.symlink_relative(&session.guard, &scoped_target, &relative, &scoped_link) {
        Ok(()) => Ok(LinkResult::Linked),
        Err(_) if req.method == InstallMethod::Copy => {
            install_copy(rt, &session.guard, dir, &req.skill, &req.files)?;
            Ok(LinkResult::Copied)
        }
        Err(e) => Err(CoreError::io(link, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `normalize_identity_strips_a_git_prefix_and_a_trailing_slash_or_names_the_mismatch`
    /// (R8): a `git:<url>` source (the shape `req.source` carries for a
    /// `Dotagents` git install - `skill_add.rs`'s `format!("git:{url}")`)
    /// normalizes to the same identity the desktop already stores for it
    /// (`normalize_git_url_identity`), so a source the desktop already
    /// trusts does not re-prompt here.
    #[test]
    fn normalize_identity_strips_a_git_prefix_and_a_trailing_slash_or_names_the_mismatch() {
        assert_eq!(
            normalize_identity("git:https://github.com/getsentry/agent-browser.git"),
            "https://github.com/getsentry/agent-browser"
        );
        assert_eq!(
            normalize_identity("Owner/Repo/"),
            "owner/repo",
            "a trailing slash and case must not produce a distinct identity"
        );
        assert_eq!(
            normalize_identity("  Owner/Repo.git  "),
            "owner/repo",
            "whitespace and a trailing .git must still be stripped, same as before R8"
        );
    }
}
