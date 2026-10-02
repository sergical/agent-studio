//! `ops::update`: refreshes one already-installed skill in place, by
//! [`InstallMethod::Copy`], `Dotagents`, or `SkillsSh` - the same three
//! methods `ops_install` writes, reusing its lease/journal/registry helpers
//! (`crate::ops_install::{journal_root, ensure_journal_root, scope_root,
//! registry_path, read_registry_document, write_registry_document,
//! method_wire_name, copy_deployment_id}`, all made `pub(crate)` for this
//! module - see that module's own doc for what each one does).
//!
//! Every method takes the per-scope exclusive lease
//! ([`crate::ports::MutationSession::begin`]), records an `update` journal
//! row - a backup of the *existing* destination (never "absent", unlike
//! install's: `update` only ever runs over a deployment already on disk) and
//! the `restore_backup` inverse that undoes it - before the first byte
//! moves, exactly the guarantee `docs/action-map/install.md` "Desired
//! state" names and `docs/action-map/remove-and-update.md` "Desired state"
//! extends to update ("record a journal event with a backup and an inverse
//! before the first write").
//!
//! `Copy` re-stages fresh `req.files` beside the destination and swaps them
//! in through [`crate::fsops::swap`], which - since a folder already sits at
//! `final_name` this time - takes its own "exchange, then move the old one
//! into `quarantine_dir`" path, so the previous tree lands in
//! [`crate::doctor::QUARANTINE_DIR_NAME`] - the same folder the doctor
//! prune and check sweep, not an update-specific name - rather than being
//! deleted. [`crate::fsops::swap`]'s `quarantine_dir` is confined under the
//! same [`crate::fsops::Root`] as `stage`/`final_name` (see that function's
//! own doc), so it cannot resolve to the desktop's separate
//! `<home>/.agents/skills-trash` without changing that primitive's contract
//! - the brief for this unit named `skills-trash` as the model location, but
//!   this reuses `fsops::swap`'s own quarantine convention instead of
//!   widening the primitive; see the unit's PR body for that deviation.
//!
//! `SkillsSh` re-runs `npx skills update <name>` and `Dotagents` re-runs
//! `npx -y @sentry/dotagents install`, in place over the existing
//! destination - not staged, for the same reason `ops_install`'s own CLI
//! methods are not: redirecting the CLI into a temporary home to force a
//! stage-and-swap would fight its own layout assumptions (see the shared
//! brief's Correction section, and `ops_install`'s module doc). The journal
//! row's backup of the destination, taken before this call, is what stands
//! in for the "old tree" a crash mid-CLI-call would otherwise lose.
//!
//! `Dotagents` never uses `dotagents add`: in dotagents 3.1.0 `add` looks
//! for plugins before skills and fails on a repo whose marketplace lists
//! `"source": "./"` (upstream getsentry/dotagents#198), and it ignores the
//! entry's `path`. The skill is already declared as a `[[skills]]` entry in
//! the scope's `agents.toml`, so an update sets that entry's `ref` (only
//! when the caller resolved a newer commit) and runs `install`, which
//! fetches whatever the entry now names. The journal row backs up the
//! folder, `agents.toml` and `agents.lock` in one call, so undo puts all
//! three back together. `install` refreshes every declared entry in that
//! scope; entries without a `ref` float to their latest commit on any
//! install - that is dotagents' own rule, not something this op adds.
//!
//! Because dotagents 3.1.0 has no targeted update, the row also backs up
//! every other skill folder `agents.toml` declares or `agents.lock` lists
//! under the Universal root, and records each one's post-write fingerprint
//! (`secondary_post`) so undo refuses over a later edit. A folder the install
//! creates is recorded for removal. Plugins and other runtime files that
//! `install` may rewrite are outside the backup: undo does not restore them.

use std::path::{Path, PathBuf};

use crate::dto::{InstallMethod, UpdateAllItem, UpdateAllOutcome, UpdateOutcome, UpdateRequest};
use crate::error::{CoreError, ErrorCode};
use crate::events::{fingerprint_path, EventDraft, EventKind, EventStatus};
use crate::fsops::{self, Root};
use crate::identity::{PlanId, RootScope, SkillName, UNIVERSAL_ROOT_RELATIVE};
use crate::journal::{FsJournal, PlanWriter};
use crate::ops::Operation;
use crate::ops_install;
use crate::ports::{
    ExclusiveGuard, FileKind, MutationSession, OpContext, PlanStatus, Runtime, ScopeFs,
};

/// The `npx` argv `update_via_cli` hands the spawner, and the process cwd to
/// run it in: skills.sh from `skill_lifecycle.rs`'s `skills_sh_update_args`
/// (`npx skills update <name> [--global]`), dotagents as `npx -y
/// @sentry/dotagents [--project] install` - never `add`, see the module doc.
/// Both run with the process cwd set to the project path for a
/// project-scope update (`commands.rs`'s `run_update_skill`:
/// `command.current_dir(project_path)`), the same fix `install`'s own
/// `cli_args_and_cwd` carries for a project-scope install (`skills@1.7.0`
/// has no `--cwd` flag). `skills update` without a scope flag means scope
/// "both", so a project update names `--project` to leave the global copy of
/// the same name alone.
fn update_cli_args_and_cwd(
    method: InstallMethod,
    skill: &SkillName,
    scope: &RootScope,
) -> (Vec<String>, Option<PathBuf>) {
    let cwd = match scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.clone()),
    };
    match method {
        InstallMethod::SkillsSh => {
            let mut args = vec!["skills".to_string(), "update".to_string(), skill.0.clone()];
            args.push(
                match scope {
                    RootScope::Global => "--global",
                    RootScope::Project(_) => "--project",
                }
                .to_string(),
            );
            (args, cwd)
        }
        InstallMethod::Dotagents => {
            let mut args = vec!["-y".to_string(), "@sentry/dotagents".to_string()];
            if matches!(scope, RootScope::Project(_)) {
                args.push("--project".to_string());
            }
            args.push("install".to_string());
            (args, cwd)
        }
        InstallMethod::Copy => (Vec::new(), None),
    }
}

/// What a `Dotagents` update decided before its journal row exists.
struct DotagentsPlan {
    /// `agents.toml`, or the file its link resolves to, so undo restores the
    /// real file and the link survives.
    config: PathBuf,
    /// `agents.lock`, or the file its link resolves to.
    lock: PathBuf,
    /// The folder holding `agents.toml` and `agents.lock`, before link resolution.
    dir: PathBuf,
    /// Names of the non-wildcard `[[skills]]` entries, which `install`
    /// refreshes along with the one being updated.
    declared: Vec<String>,
    /// `agents.toml` as read, written back when the install fails.
    original_config: String,
    /// The edited `agents.toml` text to write once the row is recorded;
    /// `None` when no new ref is pinned.
    edited_config: Option<String>,
}

/// Reads `agents.toml`, checks it declares a `[[skills]]` entry named
/// `req.skill`, and - when `req.ref_pin` is set - sets that entry's `ref`
/// with `toml_edit`, so comments and formatting survive. Writes nothing:
/// `update` calls this before `backup_paths`, so a refusal leaves no
/// journal row.
fn plan_dotagents_update(
    rt: &Runtime,
    fs: &dyn ScopeFs,
    req: &UpdateRequest,
) -> Result<DotagentsPlan, CoreError> {
    let project = match &req.scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.as_path()),
    };
    let dir = crate::dotagents_ledger::dotagents_dir(&rt.scope.home.lexical, project);
    let config = crate::ports::resolve_config_link(fs, &dir.join("agents.toml"))?;
    let text = match fs.read_capped(&config, crate::dotagents_ledger::DOTAGENTS_FILE_MAX_BYTES) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|e| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                format!("{} is not valid UTF-8: {e}", config.display()),
            )
            .at(&config)
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "{} does not exist; there is nothing to update",
                    config.display()
                ),
            )
            .at(&config))
        }
        Err(e) => return Err(CoreError::io(&config, e)),
    };
    let mut doc = text.parse::<toml_edit::DocumentMut>().map_err(|e| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            format!("{} is not valid TOML: {e}", config.display()),
        )
        .at(&config)
    })?;
    let declared: Vec<String> = doc
        .get("skills")
        .and_then(toml_edit::Item::as_array_of_tables)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("name").and_then(toml_edit::Item::as_str))
        .filter(|name| *name != "*")
        .map(str::to_string)
        .collect();
    let entry = doc
        .get_mut("skills")
        .and_then(toml_edit::Item::as_array_of_tables_mut)
        .and_then(|rows| {
            rows.iter_mut().find(|row| {
                row.get("name").and_then(toml_edit::Item::as_str) == Some(req.skill.0.as_str())
            })
        })
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "{} has no [[skills]] entry named {}; dotagents install would not update it",
                    config.display(),
                    req.skill.0
                ),
            )
            .at(&config)
        })?;
    let pinned = req.ref_pin.as_deref().map(|commit| {
        entry["ref"] = toml_edit::value(commit);
    });
    Ok(DotagentsPlan {
        config,
        lock: crate::ports::resolve_config_link(fs, &dir.join("agents.lock"))?,
        dir,
        declared,
        original_config: text,
        edited_config: pinned.map(|()| doc.to_string()),
    })
}

/// Every skill folder besides `destination` that `dotagents install` can
/// rewrite: each non-wildcard `agents.toml` entry and each `agents.lock`
/// row, as a path under `universal_root`. The lock is read before the
/// install, so a row the install is about to drop is still named here.
fn other_declared_folders(
    fs: &dyn ScopeFs,
    plan: &DotagentsPlan,
    universal_root: &Path,
    destination: &Path,
) -> Vec<PathBuf> {
    let locked = crate::dotagents_ledger::read_dotagents_ledger(fs, &plan.dir)
        .ok()
        .into_iter()
        .flatten()
        .map(|skill| skill.name);
    let mut folders: Vec<PathBuf> = Vec::new();
    for name in plan.declared.iter().cloned().chain(locked) {
        let is_plain_name =
            !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']);
        let folder = universal_root.join(&name);
        if is_plain_name && folder != destination && !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    folders
}

/// Names of the skill-shaped entries directly under `universal_root`.
fn skill_folder_names(fs: &dyn ScopeFs, universal_root: &Path) -> Result<Vec<String>, CoreError> {
    let entries = match fs.read_dir(universal_root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(CoreError::io(universal_root, e)),
    };
    Ok(entries
        .into_iter()
        .filter(crate::ports::is_skill_shaped_entry)
        .map(|entry| entry.name)
        .collect())
}

/// Patches the update row's inverse with what only the finished install can
/// say: each secondary path's post-write fingerprint (undo refuses when one
/// was edited since), and each folder the install created, which undo
/// removes.
fn record_dotagents_side_effects(
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    id: &crate::identity::EventId,
    universal_root: &Path,
    secondary: &[PathBuf],
    folders_before: &[String],
) -> Result<(), CoreError> {
    let secondary_post = secondary
        .iter()
        .map(|path| (path.clone(), fingerprint_path(fs, path)))
        .collect::<Vec<_>>();
    // A folder that cannot be listed or read is left out of `created`, never
    // allowed to drop the `secondary_post` part above.
    let mut created = Vec::new();
    for name in skill_folder_names(fs, universal_root).unwrap_or_default() {
        let folder = universal_root.join(&name);
        if folders_before.contains(&name) || secondary.contains(&folder) {
            continue;
        }
        if let Ok(Some(fingerprint)) = fingerprint_path(fs, &folder) {
            created.push((folder, fingerprint));
        }
    }
    let patch = crate::events::with_remove_copies(
        crate::events::with_secondary_post(serde_json::json!({}), &secondary_post),
        &created,
    );
    session.store.patch_inverse(&session.guard, id, patch)
}

/// The `<command> failed: <detail>` error for a non-zero CLI exit: the last
/// few stderr lines that say something, without the `npm notice`/`npm warn`
/// chatter `npx` prints around every run.
fn cli_failure(command: &str, output: &crate::ports::ProcessOutput) -> CoreError {
    let lines: Vec<&str> = output
        .stderr
        .lines()
        .map(str::trim)
        .filter(|line| {
            !line.is_empty() && !line.starts_with("npm notice") && !line.starts_with("npm warn")
        })
        .collect();
    let detail = if output.timed_out {
        "timed out".to_string()
    } else if lines.is_empty() {
        format!("exit status {:?}", output.status)
    } else {
        lines[lines.len().saturating_sub(3)..].join(" ")
    };
    CoreError::new(ErrorCode::Io, format!("{command} failed: {detail}"))
}

/// `Dotagents`/`SkillsSh` preconditions (U5): a missing source or a host
/// build with no process spawner - `update` calls this before `backup_paths`
/// records anything, so either failure leaves no journal row, matching
/// `ops_install`'s own validation order.
fn validate_cli_request(rt: &Runtime, req: &UpdateRequest) -> Result<(), CoreError> {
    if req.method == InstallMethod::Dotagents && req.source.is_none() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "a dotagents update needs a source",
        ));
    }
    if matches!(
        req.method,
        InstallMethod::Dotagents | InstallMethod::SkillsSh
    ) && rt.ports.spawner.is_none()
    {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "this host build has no process spawner; dotagents/skills.sh updates are not available",
        ));
    }
    Ok(())
}

/// `Dotagents`/`SkillsSh`: runs `req.method`'s argv (see
/// [`update_cli_args_and_cwd`]) through the process-spawner port and checks
/// the destination still exists afterward. Assumes [`validate_cli_request`]
/// already ran (`update` calls it before the first write).
fn update_via_cli(
    rt: &Runtime,
    ctx: &OpContext,
    req: &UpdateRequest,
    destination: &Path,
) -> Result<(), CoreError> {
    let spawner = rt.ports.spawner.as_ref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::Unsupported,
            "this host build has no process spawner; dotagents/skills.sh updates are not available",
        )
    })?;
    let (args, cwd) = update_cli_args_and_cwd(req.method, &req.skill, &req.scope);
    let spec = crate::ports::ProcessSpec {
        program: "npx".to_string(),
        args,
        cwd,
        env: Vec::new(),
        timeout_ms: 120_000,
    };
    let output = spawner.run(&spec, ctx.cancel.as_ref())?;
    if output.status != Some(0) {
        let command = if req.method == InstallMethod::Dotagents {
            "dotagents install"
        } else {
            "skills update"
        };
        return Err(cli_failure(command, &output));
    }
    if rt.ports.fs.symlink_metadata(destination).is_err() {
        return Err(
            CoreError::new(ErrorCode::Io, "the CLI removed the expected destination")
                .at(destination),
        );
    }
    Ok(())
}

/// The harness skills directories that already hold `<skill>` (as anything,
/// links included) before the CLI runs.
fn harness_dirs_holding(rt: &Runtime, scope: &RootScope, skill: &SkillName) -> Vec<PathBuf> {
    crate::ops::harness_own_skill_roots(rt, scope)
        .into_iter()
        .filter(|dir| rt.ports.fs.symlink_metadata(&dir.join(&skill.0)).is_ok())
        .collect()
}

/// `npx skills update` links the skill into every harness it knows, not only
/// the ones that had it. Removes each link that appeared during the update in
/// a harness folder that did not hold the skill before, so an update never
/// turns a harness on. A real folder there is not ours to delete: it stays,
/// with a warning.
fn remove_links_the_cli_added(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    scope: &RootScope,
    skill: &SkillName,
    held_before: &[PathBuf],
) -> Result<(), CoreError> {
    let fs = rt.ports.fs.as_ref();
    for dir in crate::ops::harness_own_skill_roots(rt, scope) {
        if held_before.contains(&dir) {
            continue;
        }
        let entry = dir.join(&skill.0);
        let Ok(facts) = fs.symlink_metadata(&entry) else {
            continue;
        };
        if facts.kind == FileKind::Symlink {
            let scoped = crate::ports::confine(&rt.scope, fs, &entry)?;
            fs.remove_file(guard, &scoped)
                .map_err(|e| CoreError::io(&entry, e))?;
        } else {
            // The core has no warning channel on `UpdateOutcome`; stderr is
            // the only place a CLI or desktop log picks this up.
            #[allow(clippy::print_stderr)]
            {
                eprintln!(
                "warning: skills update added {} in a harness that did not have {}; it is a real folder, so it stays",
                entry.display(),
                skill.0
            );
            }
        }
    }
    Ok(())
}

/// `Copy`: stages `files` under [`ops_install::journal_root`], then swaps it
/// into `<universal_root>/<skill>`, quarantining whatever already sat there
/// - see the module doc for why `QUARANTINE_DIR_NAME`, not the desktop's
///   `skills-trash`.
fn update_copy(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    universal_root: &Path,
    skill: &SkillName,
    files: &[crate::dto::InstallFile],
) -> Result<(), CoreError> {
    let fs = rt.ports.fs.clone();
    ops_install::ensure_journal_root(rt, guard, fs.as_ref())?;
    let journal_root = ops_install::journal_root(&rt.scope.home.lexical);
    let journal = FsJournal::new(journal_root, fs.clone());

    let root = Root::open(fs.as_ref(), universal_root.to_path_buf())
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    let plan_id = PlanId(rt.ports.ids.next_event_id().0);
    let plan = PlanWriter::begin(
        &journal,
        guard,
        plan_id,
        rt.ports.clock.now(),
        format!("update {}", skill.0),
        universal_root.to_path_buf(),
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
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    let final_name = Path::new(&skill.0);
    // Same directory the doctor prune and check sweep, not a
    // update-specific name: a quarantine folder the prune never sees would
    // grow unbounded.
    let quarantine_dir = Path::new(crate::doctor::QUARANTINE_DIR_NAME);
    fsops::swap(&root, &plan, final_name, &staged, quarantine_dir)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    plan.finish(PlanStatus::Done)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?;
    Ok(())
}

/// `Copy` only: both registry documents `write_copy_registry` will later
/// mutate, read up front - `update` calls this before `backup_paths` (U4),
/// so an unreadable registry fails before the first write, the same
/// ordering `ops_install::install`'s own registry read already uses,
/// instead of after `update_copy` has already swapped the new tree in.
struct CopyRegistryRead {
    root: PathBuf,
    document: serde_json::Map<String, serde_json::Value>,
    home_document: Option<serde_json::Map<String, serde_json::Value>>,
}

impl CopyRegistryRead {
    /// The `registry_undo` entry for this update's `copies` row, recorded
    /// with the event before the write: the old row and no `expected`, so a
    /// crash before [`write_copy_registry`]'s guarded entry is patched in
    /// still lets undo put the old `content_hash` back with the old bytes.
    fn undo_before_write(&self, rt: &Runtime, deployment_id: &str) -> Option<serde_json::Value> {
        let home_doc = self.home_document.as_ref().unwrap_or(&self.document);
        let previous = home_doc.get("copies")?.get(deployment_id)?;
        let mut entry = ops_install::guarded_registry_undo(
            &rt.scope.home.lexical,
            "copies",
            deployment_id,
            Some(previous),
            None,
        );
        entry.as_object_mut()?.remove("expected");
        Some(entry)
    }
}

fn copy_registry_id(req: &UpdateRequest, destination: &Path) -> String {
    ops_install::copy_deployment_id(
        &req.scope,
        &req.skill,
        destination,
        crate::identity::SkillDestination::Universal,
        "universal",
    )
}

fn read_copy_registry(
    rt: &Runtime,
    fs: &dyn ScopeFs,
    scope: &RootScope,
) -> Result<CopyRegistryRead, CoreError> {
    let root = ops_install::scope_root(rt, scope);
    let home_root = rt.scope.home.lexical.clone();
    let document = ops_install::read_registry_document(fs, &root)?;
    let home_document = if root == home_root {
        None
    } else {
        Some(ops_install::read_registry_document(fs, &home_root)?)
    };
    Ok(CopyRegistryRead {
        root,
        document,
        home_document,
    })
}

/// `Copy` only: writes the `content_hash` `update_copy` produced (the same
/// key `ops_install::install_and_link` writes on first install, R1/R2
/// there) into the documents `read_copy_registry` already pulled before the
/// first write - only this write itself has to wait for the swap, since the
/// hash it records depends on the bytes the swap just landed.
fn write_copy_registry(
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    rt: &Runtime,
    req: &UpdateRequest,
    destination: &Path,
    mut read: CopyRegistryRead,
    content_hash: String,
) -> Result<Option<serde_json::Value>, CoreError> {
    let deployment_id = copy_registry_id(req, destination);
    let home_doc = read.home_document.as_mut().unwrap_or(&mut read.document);
    let mut undo = None;
    if let Some(serde_json::Value::Object(copies)) = home_doc.get_mut("copies") {
        if let Some(entry) = copies.get_mut(&deployment_id) {
            let previous = entry.clone();
            entry["content_hash"] = serde_json::Value::String(content_hash);
            undo = Some(ops_install::guarded_registry_undo(
                &rt.scope.home.lexical,
                "copies",
                &deployment_id,
                Some(&previous),
                Some(&*entry),
            ));
        }
    }
    if let Some(home_document) = read.home_document {
        ops_install::write_registry_document(
            &session.guard,
            fs,
            &rt.scope.home.lexical,
            home_document,
        )?;
    } else {
        ops_install::write_registry_document(&session.guard, fs, &read.root, read.document)?;
    }
    Ok(undo)
}

/// The write step every `update` call shares, once its journal row is
/// already recorded: writes `req.method`'s fresh bytes over the existing
/// destination. Any failure here bubbles up so `update` can mark the row
/// `Failed`, matching `ops_install`'s F9. `copy_registry` is `Some` only for
/// `Copy` - `update` reads it before the first write (U4) and hands it here
/// to be written back once the swap has landed.
#[allow(clippy::too_many_arguments)]
fn update_write(
    rt: &Runtime,
    ctx: &OpContext,
    session: &mut MutationSession,
    req: &UpdateRequest,
    universal_root: &Path,
    destination: &Path,
    copy_registry: Option<CopyRegistryRead>,
    dotagents: Option<&DotagentsPlan>,
) -> Result<Option<serde_json::Value>, CoreError> {
    let fs = rt.ports.fs.as_ref();
    match req.method {
        InstallMethod::Copy => {
            update_copy(rt, &session.guard, universal_root, &req.skill, &req.files)?;
            let content_hash = crate::ops::skill_content_hash(fs, ctx, destination)?;
            let read = copy_registry.ok_or_else(|| {
                CoreError::new(
                    ErrorCode::Io,
                    "a copy update reached its write step with no pre-read registry documents",
                )
            })?;
            write_copy_registry(session, fs, rt, req, destination, read, content_hash)
        }
        InstallMethod::Dotagents | InstallMethod::SkillsSh => {
            if let Some(plan) = dotagents {
                if let Some(text) = &plan.edited_config {
                    // `plan.config` is already the file a link resolves to,
                    // so a linked `agents.toml` keeps its link.
                    let scoped = crate::ports::confine_write_through(&rt.scope, fs, &plan.config)?;
                    fs.write_atomic(&session.guard, &scoped, text.as_bytes())
                        .map_err(|e| CoreError::io(&plan.config, e))?;
                }
            }
            let held_before = harness_dirs_holding(rt, &req.scope, &req.skill);
            let refreshed = update_via_cli(rt, ctx, req, destination).and_then(|()| {
                if req.method == InstallMethod::SkillsSh {
                    remove_links_the_cli_added(
                        rt,
                        &session.guard,
                        &req.scope,
                        &req.skill,
                        &held_before,
                    )?;
                }
                Ok(())
            });
            if let Err(e) = refreshed {
                if let Some(plan) = dotagents.filter(|p| p.edited_config.is_some()) {
                    // Best effort: the original error is the one to report.
                    let _ = crate::ports::confine_write_through(&rt.scope, fs, &plan.config).map(
                        |scoped| {
                            fs.write_atomic(
                                &session.guard,
                                &scoped,
                                plan.original_config.as_bytes(),
                            )
                        },
                    );
                }
                return Err(e);
            }
            Ok(None)
        }
    }
}

/// Refreshes one already-installed skill by `req.method`, under the
/// exclusive lease over `req.scope`'s root - see the module doc for the
/// write shape each method takes.
pub fn update(
    rt: &Runtime,
    ctx: &OpContext,
    req: &UpdateRequest,
) -> Result<UpdateOutcome, CoreError> {
    rt.run(Operation::Update, ctx, || update_body(rt, ctx, req))
}

fn update_body(
    rt: &Runtime,
    ctx: &OpContext,
    req: &UpdateRequest,
) -> Result<UpdateOutcome, CoreError> {
    ctx.checkpoint()?;
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let session = MutationSession::begin(rt, ctx);
    ctx.take_timing();
    let mut session = session?;
    let begin_step = crate::timing::step(clock, "begin_session", step_start);

    let fs = rt.ports.fs.as_ref();
    let root = ops_install::scope_root(rt, &req.scope);
    let universal_root = root.join(UNIVERSAL_ROOT_RELATIVE);
    let destination = universal_root.join(&req.skill.0);
    if fs.symlink_metadata(&destination).is_err() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "update needs an existing copy; none exists at this destination",
        )
        .at(&destination));
    }
    let tree_hash_before = crate::tree_hash::tree_hash(fs, &destination)?;

    // U5: both checks run before `backup_paths`, so a missing source or a
    // spawner-less host build leaves no journal row.
    validate_cli_request(rt, req)?;
    // U4: `Copy`'s registry documents are read here too, before the first
    // write, so an unreadable registry fails the same way - see
    // `read_copy_registry`'s own doc.
    let copy_registry = match req.method {
        InstallMethod::Copy => Some(read_copy_registry(rt, fs, &req.scope)?),
        InstallMethod::Dotagents | InstallMethod::SkillsSh => None,
    };
    // Same ordering for `Dotagents`: `agents.toml` is read, checked and
    // edited in memory here, so a missing entry fails before any row.
    let dotagents = match req.method {
        InstallMethod::Dotagents => Some(plan_dotagents_update(rt, fs, req)?),
        InstallMethod::Copy | InstallMethod::SkillsSh => None,
    };

    let step_start = clock.monotonic();
    let id = rt.ports.ids.next_event_id();
    // The row goes down before the first write, same as install's own F7 -
    // this time the backup captures the real tree already on disk (never
    // "absent": `update` refuses above when nothing is there yet), which is
    // what the crash-window test and `ops::restore_event` undo against. The
    // destination stays first: its entry is the row's primary path, and
    // `restore_event` puts every other entry - `Dotagents`' `agents.toml`
    // and `agents.lock` - back beside it.
    let mut backup_targets = vec![destination.clone()];
    let mut folders_before = Vec::new();
    if let Some(plan) = &dotagents {
        backup_targets.push(plan.config.clone());
        backup_targets.push(plan.lock.clone());
        backup_targets.extend(other_declared_folders(
            fs,
            plan,
            &universal_root,
            &destination,
        ));
        folders_before = skill_folder_names(fs, &universal_root)?;
    }
    let manifest = session
        .store
        .backup_paths(&session.guard, &id, &backup_targets)?;
    // `pre` is the backup's own fingerprint of the tree `update` is about to
    // overwrite - never `None` here, since `update` already refused above
    // when the destination did not exist. `None` would tell `restore_event`
    // the path was absent before this event, which would make undo *remove*
    // the restored tree instead of writing it back.
    let pre_fingerprint = manifest
        .entries
        .first()
        .and_then(|e| e.fingerprint.as_ref());
    let mut inverse = crate::events::restore_backup_inverse(&destination, pre_fingerprint, None);
    if let Some(entry) = copy_registry
        .as_ref()
        .and_then(|read| read.undo_before_write(rt, &copy_registry_id(req, &destination)))
    {
        inverse["registry_undo"] = serde_json::json!([entry]);
    }
    let draft = EventDraft {
        kind: EventKind::Update,
        skill: req.skill.clone(),
        harness: None,
        scope: Some(crate::ops::scope_label(&req.scope).to_string()),
        project_path: match &req.scope {
            RootScope::Global => None,
            RootScope::Project(p) => Some(p.0.clone()),
        },
        payload: serde_json::json!({
            "method": ops_install::method_wire_name(req.method),
            "destination": destination,
            "source": req.source,
            "tree_hash_before": tree_hash_before,
        }),
        inverse: Some(inverse),
        backup_dir: Some(manifest.backup_dir.clone()),
    };
    session.store.record(&session.guard, &id, &draft)?;

    let registry_undo = match update_write(
        rt,
        ctx,
        &mut session,
        req,
        &universal_root,
        &destination,
        copy_registry,
        dotagents.as_ref(),
    ) {
        Ok(undo) => undo,
        Err(e) => {
            let _ = session
                .store
                .finish(&session.guard, &id, EventStatus::Failed, None);
            return Err(e);
        }
    };
    if let Some(entry) = registry_undo {
        // Adds `expected` to the entry recorded before the write, so undo
        // refuses when the row changed since. Only that row: the file holds
        // every other skill's row too.
        let _ = session.store.patch_inverse(
            &session.guard,
            &id,
            serde_json::json!({ "registry_undo": [entry] }),
        );
    }
    if dotagents.is_some() {
        // Best-effort like the registry patch above: the install already
        // wrote, so the row must still finish for undo to find it.
        let _ = record_dotagents_side_effects(
            &mut session,
            fs,
            &id,
            &universal_root,
            &backup_targets[1..],
            &folders_before,
        );
    }
    let tree_hash_after = crate::tree_hash::tree_hash(fs, &destination)?;
    // The post-fingerprint the row records, not `None`: `restore_event`
    // compares the live tree against this on undo (`expected =
    // post.unwrap_or("absent")`), so leaving it `None` would tell undo the
    // path was absent after this event and turn a plain restore into
    // `DriftConflict` against the tree `update` just wrote.
    let post_fingerprint = fingerprint_path(fs, &destination)?;
    session
        .store
        .finish(&session.guard, &id, EventStatus::Done, post_fingerprint)?;
    session.finish(rt, ctx);
    let write_step = crate::timing::step(clock, "write", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "update",
        op_start,
        vec![begin_step, write_step],
    ));
    Ok(UpdateOutcome {
        event_id: id,
        skill: req.skill.clone(),
        deployment_path: destination,
        tree_hash_before,
        tree_hash_after,
    })
}

/// Runs [`update`] once per entry in `requests`, each its own journal row
/// (`update`'s own lease/journal shape, taken and released per call - no
/// batch-wide lease), calling `on_outcome` as each one finishes so a caller
/// (the desktop's "update all") can update its list in place without
/// waiting for the whole batch - no UI-thread work is this crate's concern;
/// which thread a caller runs this loop on is tested where that caller
/// lives, per this unit's split.
pub fn update_all(
    rt: &Runtime,
    ctx: &OpContext,
    requests: &[UpdateRequest],
    mut on_outcome: impl FnMut(&SkillName, &Result<UpdateOutcome, CoreError>),
) -> UpdateAllOutcome {
    // Infallible: each `update` call already files its own `Operation::Update`
    // record (`Ok` or `Err`) as a nested op under this one, so this batch's
    // own record only needs to exist - it always reports `Ok`, with its
    // timing filed by `update_all_body` itself, spanning the whole loop.
    match rt.run(Operation::UpdateAll, ctx, || {
        Ok(update_all_body(rt, ctx, requests, &mut on_outcome))
    }) {
        Ok(outcome) => outcome,
        Err(_) => unreachable!("update_all_body never returns Err"),
    }
}

fn update_all_body(
    rt: &Runtime,
    ctx: &OpContext,
    requests: &[UpdateRequest],
    on_outcome: &mut impl FnMut(&SkillName, &Result<UpdateOutcome, CoreError>),
) -> UpdateAllOutcome {
    let clock = rt.ports.clock.as_ref();
    let start = clock.monotonic();
    let mut items = Vec::with_capacity(requests.len());
    let mut errors = std::collections::BTreeMap::new();
    for req in requests {
        let result = update(rt, ctx, req);
        on_outcome(&req.skill, &result);
        match result {
            Ok(outcome) => items.push(UpdateAllItem {
                skill: req.skill.clone(),
                outcome: Some(outcome),
            }),
            Err(e) => {
                errors.insert(req.skill.0.clone(), e.message.clone());
                items.push(UpdateAllItem {
                    skill: req.skill.clone(),
                    outcome: None,
                });
            }
        }
    }
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "update_all",
        start,
        Vec::new(),
    ));
    UpdateAllOutcome { items, errors }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::ProjectRef;

    /// `project_skills_sh_update_names_the_project_scope_or_also_updates_the_global_copy`:
    /// `skills update <name>` with no scope flag means scope "both" in
    /// skills 1.7.0, so it also rewrites `~/.agents/skills/<name>`. A
    /// project-scope update must pass `--project` and never `--global`. Fails
    /// when the flag is missing: the global copy of the same name changes too.
    #[test]
    fn project_skills_sh_update_names_the_project_scope_or_also_updates_the_global_copy() {
        let skill = SkillName("alpha".to_string());
        let project = RootScope::Project(ProjectRef(PathBuf::from("/proj")));

        let (args, _) = update_cli_args_and_cwd(InstallMethod::SkillsSh, &skill, &project);

        assert!(args.contains(&"--project".to_string()), "argv: {args:?}");
        assert!(!args.contains(&"--global".to_string()), "argv: {args:?}");
    }

    /// `update_cli_args_and_cwd_builds_skills_update_or_dotagents_install_and_never_dotagents_add`:
    /// table test over {global, project} x {`SkillsSh`, `Dotagents`}. Fails
    /// if dotagents ever goes back to `add`, which breaks on repos whose
    /// marketplace lists `"source": "./"`, or if the project scope loses its
    /// `--project` flag or its cwd.
    #[test]
    fn update_cli_args_and_cwd_builds_skills_update_or_dotagents_install_and_never_dotagents_add() {
        let skill = SkillName("alpha".to_string());
        let project = RootScope::Project(ProjectRef(PathBuf::from("/proj")));

        type Case<'a> = (
            &'a str,
            InstallMethod,
            &'a RootScope,
            Vec<&'a str>,
            Option<PathBuf>,
        );
        let cases: Vec<Case> = vec![
            (
                "skills.sh global",
                InstallMethod::SkillsSh,
                &RootScope::Global,
                vec!["skills", "update", "alpha", "--global"],
                None,
            ),
            (
                "skills.sh project",
                InstallMethod::SkillsSh,
                &project,
                vec!["skills", "update", "alpha", "--project"],
                Some(PathBuf::from("/proj")),
            ),
            (
                "dotagents global",
                InstallMethod::Dotagents,
                &RootScope::Global,
                vec!["-y", "@sentry/dotagents", "install"],
                None,
            ),
            (
                "dotagents project",
                InstallMethod::Dotagents,
                &project,
                vec!["-y", "@sentry/dotagents", "--project", "install"],
                Some(PathBuf::from("/proj")),
            ),
        ];

        for (label, method, scope, expected_args, expected_cwd) in cases {
            let (args, cwd) = update_cli_args_and_cwd(method, &skill, scope);
            let expected_args: Vec<String> = expected_args.into_iter().map(String::from).collect();
            assert_eq!(args, expected_args, "{label}: argv");
            assert_eq!(cwd, expected_cwd, "{label}: cwd");
        }
    }
}
