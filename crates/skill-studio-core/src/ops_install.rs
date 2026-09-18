//! `ops::install`: puts one skill on disk by [`InstallMethod::Copy`],
//! `Dotagents`, or `SkillsSh`, all through one write path.
//!
//! Every method takes the per-scope exclusive lease
//! ([`crate::ports::MutationSession::begin`]), then records an `install`
//! journal row (backup and inverse, per `docs/action-map/install.md`
//! "Desired state") before the first byte moves. `Copy` stages its folder
//! beside the destination and swaps it into place through
//! [`crate::fsops`]'s `stage`/`swap`, journaled by a dedicated
//! [`crate::journal::FsJournal`] rooted under the same scope - nothing in
//! [`crate::ports::Ports`] wires one in yet, so this op opens its own, the
//! way `docs/action-map/plan.md`'s Correction section for unit 3.5 asks for.
//! `Dotagents`/`SkillsSh` call `npx ... add <source>` through the
//! process-spawner port and let the CLI write its own files; this op
//! records the destination's backup and the install's inverse before that
//! call, but does not stage-and-swap the CLI's own writes - redirecting it
//! into a temporary home to force that would fight the CLI's own layout
//! assumptions, per the same Correction section.
//!
//! Trust: a `Dotagents`/`SkillsSh` source carrying a `trust_identity` must
//! already be trusted (recorded by an earlier confirmed install) or the
//! call must itself set `trust_confirmed`; otherwise nothing is written and
//! [`InstallOutcome::NeedsTrust`] is returned. Ported from the desktop's
//! `skill_trust_policy.rs`, minus its `WriteLeaseGuard`-specific entry
//! points (this op always already holds the exclusive lease itself).
//!
//! Linking: only Claude Code gets an explicit per-skill link from this op,
//! matching the desktop's `add_skill` (`skill_add.rs`'s doc comment: "then,
//! for `dotagents`/`copy` ..., symlinks the new skill into
//! `~/.claude/skills`"); every other harness reads the universal root
//! directly.
//!
//! Preferences: `install` (when `save_as_preference`) and
//! [`install_preferences`] read and write `preferred_method`/
//! `preferred_harnesses` in `<scope>/.agents/skill-studio.json`, merged in
//! alongside whatever other top-level keys that document already carries -
//! `serde_json`'s `preserve_order` feature and `Map::shift_remove` (never
//! `Map::remove`) keep an untouched key's position stable across a write.

use std::path::{Path, PathBuf};

use crate::dto::{InstallFile, InstallMethod, InstallOutcome, InstallPreferences, InstallRequest};
use crate::error::{CoreError, ErrorCode};
use crate::events::{EventDraft, EventKind, EventStatus};
use crate::fsops::{self, Root};
use crate::identity::{AgentId, RootScope, SkillName, UNIVERSAL_ROOT_RELATIVE};
use crate::journal::{self, FsJournal, PlanWriter};
use crate::ports::{
    self, ExclusiveGuard, FileKind, MutationSession, OpContext, PlanStatus, ProcessSpec, Runtime,
    ScopeFs,
};

/// `<scope>/.agents/skill-studio-journal` - the `FsJournal` root this op
/// opens for `Copy`'s stage/swap. Distinct from `.agents/skills` (the
/// universal root itself) so a journal plan directory never looks like an
/// installed skill to `scan`.
const INSTALL_JOURNAL_RELATIVE: &str = ".agents/skill-studio-journal";

/// `<scope>/.agents/skill-studio.json` - the registry document `install`
/// and [`install_preferences`] read and write. Matches
/// `crate::ownership::skill_studio_json_path`, but only ever addressed
/// relative to the scope this op targets (home or one project), never the
/// scope home unconditionally the way ownership classification reads it.
fn registry_path(scope_root: &Path) -> PathBuf {
    scope_root.join(".agents").join("skill-studio.json")
}

fn scope_root(rt: &Runtime, scope: &RootScope) -> PathBuf {
    match scope {
        RootScope::Global => rt.scope.home.lexical.clone(),
        RootScope::Project(project) => project.0.clone(),
    }
}

/// Reads `<scope_root>/.agents/skill-studio.json` as a JSON object, or an
/// empty one when it is missing, unreadable, or not an object - the same
/// "downgrade to nothing recorded" a missing registry gets elsewhere in
/// this crate (`crate::ownership::read_home_registry`).
fn read_registry_document(
    fs: &dyn ScopeFs,
    scope_root: &Path,
) -> serde_json::Map<String, serde_json::Value> {
    let path = registry_path(scope_root);
    let Ok(bytes) = fs.read_capped(&path, 8 * 1024 * 1024) else {
        return serde_json::Map::new();
    };
    match serde_json::from_slice::<serde_json::Value>(&bytes) {
        Ok(serde_json::Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    }
}

/// Writes `document` back to `<scope_root>/.agents/skill-studio.json`,
/// under the caller's already-held exclusive lease.
fn write_registry_document(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    fs: &dyn ScopeFs,
    scope_root: &Path,
    document: &serde_json::Map<String, serde_json::Value>,
) -> Result<(), CoreError> {
    let path = registry_path(scope_root);
    if let Some(parent) = path.parent() {
        let scoped_parent = ports::confine(&rt.scope, fs, parent)?;
        fs.create_dir_all(guard, &scoped_parent)
            .map_err(|e| CoreError::io(parent, e))?;
    }
    let bytes =
        serde_json::to_vec_pretty(&serde_json::Value::Object(document.clone())).map_err(|e| {
            CoreError::new(ErrorCode::Io, format!("failed to serialize registry: {e}")).at(&path)
        })?;
    let scoped = ports::confine(&rt.scope, fs, &path)?;
    fs.write_atomic(guard, &scoped, &bytes)
        .map_err(|e| CoreError::io(&path, e))
}

/// Normalizes a `trust_identity` for lookup/storage: trims whitespace,
/// drops a trailing `.git`, lowercases. Mirrors the desktop's
/// `skill_trust_policy::normalize_confirmation_identity`, minus the
/// multi-line/empty rejection (an empty `trust_identity` is `None`, not a
/// value this op ever sees).
fn normalize_identity(identity: &str) -> String {
    identity
        .trim()
        .trim_end_matches(".git")
        .to_ascii_lowercase()
}

/// True when `identity` is already recorded as trusted in the registry
/// document's `trusted_sources` array.
fn is_trusted(document: &serde_json::Map<String, serde_json::Value>, identity: &str) -> bool {
    document
        .get("trusted_sources")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|list| list.iter().any(|v| v.as_str() == Some(identity)))
}

/// Adds `identity` to the registry document's `trusted_sources` array,
/// deduplicated, preserving every other key already in `document`.
fn record_trusted(document: &mut serde_json::Map<String, serde_json::Value>, identity: &str) {
    let mut list: Vec<String> = document
        .get("trusted_sources")
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
        "trusted_sources".to_string(),
        serde_json::Value::Array(list.into_iter().map(serde_json::Value::String).collect()),
    );
}

fn method_wire_name(method: InstallMethod) -> &'static str {
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
    _ctx: &OpContext,
    scope: &RootScope,
) -> Result<InstallPreferences, CoreError> {
    let root = scope_root(rt, scope);
    let fs = rt.ports.fs.as_ref();
    let document = read_registry_document(fs, &root);
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

/// Installs one skill by `req.method`, under the exclusive lease over
/// `req.scope`'s root - see the module doc for the write shape each method
/// takes.
pub fn install(
    rt: &Runtime,
    ctx: &OpContext,
    req: &InstallRequest,
) -> Result<InstallOutcome, CoreError> {
    ctx.checkpoint()?;
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let session = MutationSession::begin(rt, ctx);
    ctx.take_timing();
    let mut session = session?;
    let begin_step = crate::timing::step(clock, "begin_session", step_start);

    let fs = rt.ports.fs.as_ref();
    let root = scope_root(rt, &req.scope);
    let mut document = read_registry_document(fs, &root);

    if let Some(identity) = &req.trust_identity {
        let normalized = normalize_identity(identity);
        if !req.trust_confirmed && !is_trusted(&document, &normalized) {
            return Ok(InstallOutcome::NeedsTrust {
                identity: normalized,
            });
        }
        if req.trust_confirmed {
            record_trusted(&mut document, &normalized);
        }
    }

    let universal_root = root.join(UNIVERSAL_ROOT_RELATIVE);
    crate::ops::ensure_dir_all(rt, &session, fs, &universal_root)?;
    let destination = universal_root.join(&req.skill.0);
    if fs.symlink_metadata(&destination).is_ok() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "a deployment already exists at this destination; install does not overwrite one",
        )
        .at(&destination));
    }

    let step_start = clock.monotonic();
    let id = rt.ports.ids.next_event_id();
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
        }),
        // Undo of an install is not implemented by this build, matching
        // `ops::park`'s own `inverse: None` - restore refuses an `Install`
        // row with `ErrorCode::Unsupported` rather than guess at removal.
        inverse: None,
        backup_dir: None,
    };
    session.store.record(&session.guard, &id, &draft)?;

    let write: Result<(), CoreError> = match req.method {
        InstallMethod::Copy => install_copy(
            rt,
            &session.guard,
            &root,
            &universal_root,
            &req.skill,
            &req.files,
        ),
        InstallMethod::Dotagents | InstallMethod::SkillsSh => install_via_cli(
            rt,
            ctx,
            req.method,
            req.source.as_deref(),
            &root,
            &destination,
        ),
    };
    if let Err(e) = write {
        let _ = session
            .store
            .finish(&session.guard, &id, EventStatus::Failed, None);
        return Err(e);
    }

    let mut linked = Vec::new();
    if req
        .harnesses
        .iter()
        .any(|h| h.as_str() == AgentId::CLAUDE_CODE)
    {
        link_claude_code(rt, &mut session, fs, &root, &req.skill, &destination)?;
        linked.push(AgentId::from(AgentId::CLAUDE_CODE));
    }

    if req.save_as_preference {
        document.insert(
            "preferred_method".to_string(),
            serde_json::Value::String(method_wire_name(req.method).to_string()),
        );
        document.insert(
            "preferred_harnesses".to_string(),
            serde_json::to_value(&req.harnesses).unwrap_or(serde_json::Value::Array(Vec::new())),
        );
    }
    if req.method == InstallMethod::Copy {
        let copies = document
            .entry("copies".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let serde_json::Value::Object(copies) = copies {
            copies.insert(
                req.skill.0.clone(),
                serde_json::json!({
                    "name": req.skill.0,
                    "path": destination,
                    "scope": crate::ops::scope_label(&req.scope),
                    "destination": "universal",
                    "project_path": match &req.scope {
                        RootScope::Global => None,
                        RootScope::Project(p) => Some(p.0.clone()),
                    },
                    "content_hash": "",
                    "disabled": false,
                }),
            );
        }
    }
    write_registry_document(rt, &session.guard, fs, &root, &document)?;

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
        linked_harnesses: linked,
    })
}

/// `Copy`: stages `files` under a dedicated [`FsJournal`] rooted at
/// [`INSTALL_JOURNAL_RELATIVE`], then swaps the staged folder into
/// `<universal_root>/<skill>`. Sweeps any plan a previous crash left
/// `Pending` first, so a stray staged folder from an earlier interrupted
/// install never accumulates.
fn install_copy(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    scope_root: &Path,
    universal_root: &Path,
    skill: &SkillName,
    files: &[InstallFile],
) -> Result<(), CoreError> {
    let fs = rt.ports.fs.clone();
    let journal_root = scope_root.join(INSTALL_JOURNAL_RELATIVE);
    let scoped_journal_root = ports::confine(&rt.scope, fs.as_ref(), &journal_root)?;
    fs.create_dir_all(guard, &scoped_journal_root)
        .map_err(|e| CoreError::io(&journal_root, e))?;
    let journal = FsJournal::new(journal_root, fs.clone());
    journal::reconcile(&journal, guard, fs.as_ref()).map_err(|e| {
        CoreError::new(
            ErrorCode::Io,
            format!("could not reconcile an earlier interrupted install: {e}"),
        )
    })?;

    let root = Root::open(fs.as_ref(), universal_root.to_path_buf())
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    let plan_id = crate::identity::PlanId(rt.ports.ids.next_event_id().0);
    let plan = PlanWriter::begin(
        &journal,
        guard,
        plan_id,
        rt.ports.clock.now(),
        format!("install {}", skill.0),
        universal_root.to_path_buf(),
        Vec::new(),
    )
    .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?;

    let contents: Vec<(PathBuf, Vec<u8>)> = files
        .iter()
        .map(|f| (f.relative_path.clone(), f.contents.clone()))
        .collect();
    let staged = fsops::stage(&root, &plan, &contents)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    let final_name = Path::new(&skill.0);
    let quarantine_dir = Path::new(".skill-studio-install-quarantine");
    fsops::swap(&root, &plan, final_name, &staged, quarantine_dir)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    plan.finish(PlanStatus::Done)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?;
    Ok(())
}

/// `Dotagents`/`SkillsSh`: runs `npx -y <package> add <source>` through the
/// process-spawner port, in `scope_root`, and checks the destination now
/// exists. The CLI writes its own files directly - see the module doc for
/// why this op does not stage-and-swap them.
fn install_via_cli(
    rt: &Runtime,
    ctx: &OpContext,
    method: InstallMethod,
    source: Option<&str>,
    scope_root: &Path,
    destination: &Path,
) -> Result<(), CoreError> {
    let Some(source) = source else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "a dotagents/skills.sh install needs a source",
        ));
    };
    let spawner = rt.ports.spawner.as_ref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::Unsupported,
            "this host build has no process spawner; dotagents/skills.sh installs are not available",
        )
    })?;
    let args = match method {
        InstallMethod::Dotagents => vec![
            "-y".to_string(),
            "@sentry/dotagents".to_string(),
            "add".to_string(),
            source.to_string(),
        ],
        InstallMethod::SkillsSh => vec![
            "-y".to_string(),
            "skills".to_string(),
            "add".to_string(),
            source.to_string(),
        ],
        InstallMethod::Copy => unreachable!("install_via_cli is never called for Copy"),
    };
    let spec = ProcessSpec {
        program: "npx".to_string(),
        args,
        cwd: Some(scope_root.to_path_buf()),
        env: Vec::new(),
        timeout_ms: 120_000,
    };
    let output = spawner.run(&spec, ctx.cancel.as_ref())?;
    if output.status != Some(0) {
        return Err(CoreError::new(
            ErrorCode::Io,
            format!("npx exited with {:?}: {}", output.status, output.stderr),
        ));
    }
    if rt.ports.fs.symlink_metadata(destination).is_err() {
        return Err(CoreError::new(
            ErrorCode::Io,
            "the CLI did not create the expected destination",
        )
        .at(destination));
    }
    Ok(())
}

/// Symlinks `<scope>/.claude/skills/<skill>` to `destination`, unless
/// `.claude/skills` is already a whole-directory link into the shared root
/// (every skill is already visible through it) - mirrors the guard in
/// `ops::set_claude_code_switch`.
fn link_claude_code(
    rt: &Runtime,
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    scope_root: &Path,
    skill: &SkillName,
    destination: &Path,
) -> Result<(), CoreError> {
    let claude_skills_dir = scope_root.join(".claude").join("skills");
    if fs
        .symlink_metadata(&claude_skills_dir)
        .is_ok_and(|f| f.kind == FileKind::Symlink)
    {
        return Ok(());
    }
    crate::ops::ensure_dir_all(rt, session, fs, &claude_skills_dir)?;
    let link_path = claude_skills_dir.join(&skill.0);
    let scoped_target = ports::confine(&rt.scope, fs, destination)?;
    let scoped_link = ports::confine(&rt.scope, fs, &link_path)?;
    fs.symlink(&session.guard, &scoped_target, &scoped_link)
        .map_err(|e| CoreError::io(&link_path, e))
}
