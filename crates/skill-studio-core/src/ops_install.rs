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

use crate::dto::{InstallFile, InstallMethod, InstallOutcome, InstallPreferences, InstallRequest};
use crate::error::{CoreError, ErrorCode};
use crate::events::{EventDraft, EventKind, EventStatus};
use crate::fsops::{self, Root};
use crate::identity::{AgentId, RootScope, SkillName, UNIVERSAL_ROOT_RELATIVE};
use crate::journal::{FsJournal, PlanWriter};
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
/// via `ops::deployment_id` - `slot` and `destination` are always `universal`
/// for a `Copy` install, since this op only ever writes the shared universal
/// root (see the module doc, "Linking").
pub(crate) fn copy_deployment_id(
    scope: &RootScope,
    skill: &SkillName,
    destination: &Path,
) -> String {
    let scope_label = crate::ops::scope_label(scope);
    let project_path = match scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.to_string_lossy()),
    };
    crate::ops::deployment_id(
        &skill.0,
        scope_label,
        crate::identity::SkillDestination::Universal,
        "universal",
        project_path.as_deref(),
        destination,
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
    _ctx: &OpContext,
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

/// Targets `install_and_link` writes to - bundled so the function stays
/// under clippy's argument-count lint.
struct InstallTargets<'a> {
    root: &'a Path,
    universal_root: &'a Path,
    destination: &'a Path,
    /// `<root>/.claude/skills/<skill>`, when `req.harnesses` names Claude
    /// Code - computed once, from the request alone, so the same path can
    /// be named in the journal row's inverse before any write and reused
    /// for the actual link afterward.
    claude_link_path: Option<&'a Path>,
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
    // Before anything below creates so much as a directory: `ensure_dir_all`
    // (further down, via `install_and_link`) `mkdir -p`s
    // `<project>/.agents/skills`, which would silently create a missing
    // project directory as a side effect and mask this exact fault.
    validate_cli_project_path(rt, req)?;
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let session = MutationSession::begin(rt, ctx);
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

    let universal_root = root.join(UNIVERSAL_ROOT_RELATIVE);
    let destination = universal_root.join(&req.skill.0);
    if fs.symlink_metadata(&destination).is_ok() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "a deployment already exists at this destination; install does not overwrite one",
        )
        .at(&destination));
    }
    let claude_link_requested = req
        .harnesses
        .iter()
        .any(|h| h.as_str() == AgentId::CLAUDE_CODE);
    let claude_link_path =
        claude_link_requested.then(|| root.join(".claude").join("skills").join(&req.skill.0));

    let step_start = clock.monotonic();
    let id = rt.ports.ids.next_event_id();
    // The row goes down before the first byte moves (F7): its backup is
    // whatever currently sits at the destination and the Claude Code link
    // path - normally nothing, which `backup_paths` records as "absent",
    // itself the pre-state a later restore compares against - and its
    // inverse describes undoing a completed install. `claude_link_path` is
    // already known from the request alone, so this doesn't need to wait
    // for `link_claude_code` to actually run.
    let mut backup_targets = vec![destination.clone()];
    if let Some(link) = &claude_link_path {
        backup_targets.push(link.clone());
    }
    let manifest = session
        .store
        .backup_paths(&session.guard, &id, &backup_targets)?;
    // R6: the shared `restore_backup` shape every other write-then-record op
    // uses - not a one-off `remove_install` shape nothing parses (`events.rs`
    // only recognizes `restore_backup`/`recreate_symlink`/`remove_symlink`).
    // `pre` is always `None` (absent): `destination` was checked above to
    // not exist yet, so `backup_paths` already recorded it as "absent" in
    // the manifest this inverse's `backup_dir` points at. `post` is `None`
    // too, matching every other pre-mutation inverse in this crate
    // (`ops.rs`'s own `restore_backup_inverse` call sites) - the bytes this
    // write is about to produce aren't known yet at this point.
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
        }),
        inverse: Some(inverse),
        backup_dir: Some(manifest.backup_dir.clone()),
    };
    session.store.record(&session.guard, &id, &draft)?;

    let targets = InstallTargets {
        root: &root,
        universal_root: &universal_root,
        destination: &destination,
        claude_link_path: claude_link_path.as_deref(),
    };
    // F9: `ensure_dir_all`, the write itself, the Claude Code link, and the
    // registry write all share this one fallible step, so any of their
    // failures - not just the write's - marks the row `Failed` instead of
    // leaving it `Pending`.
    let documents = RegistryDocuments {
        scope: document,
        home: home_document,
    };
    match install_and_link(rt, ctx, &mut session, fs, req, &targets, documents) {
        Err(e) => {
            let _ = session
                .store
                .finish(&session.guard, &id, EventStatus::Failed, None);
            Err(e)
        }
        Ok(linked) => {
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
    }
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
/// row is already recorded: creates `targets.universal_root`, writes
/// `req.method`'s bytes, links Claude Code when requested, and writes the
/// registry document(s) back - any failure here bubbles up so `install` can
/// mark the row `Failed` (F9).
fn install_and_link(
    rt: &Runtime,
    ctx: &OpContext,
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    req: &InstallRequest,
    targets: &InstallTargets,
    documents: RegistryDocuments,
) -> Result<Vec<AgentId>, CoreError> {
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
    crate::ops::ensure_dir_all(rt, session, fs, targets.universal_root)?;

    match req.method {
        InstallMethod::Copy => install_copy(
            rt,
            &session.guard,
            targets.universal_root,
            &req.skill,
            &req.files,
        )?,
        InstallMethod::Dotagents | InstallMethod::SkillsSh => {
            install_via_cli(rt, ctx, req, targets.destination)?;
        }
    }

    let mut linked = Vec::new();
    if let Some(link_path) = targets.claude_link_path {
        link_claude_code(rt, session, fs, targets.destination, link_path)?;
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
        // R1: keyed by the deployment id, not the skill name - every
        // consumer (`ops.rs::classify_owner`'s `home_registry.copies.get(cx.id)`,
        // the desktop's `commands.rs`/`skill_harness_disable.rs`) looks this
        // map up by id, never by name. R2: a non-empty `content_hash` - the
        // desktop's `CopyDeploymentRecord` doc says empty is legacy-only,
        // and destructive mutations refuse it.
        let deployment_id = copy_deployment_id(&req.scope, &req.skill, targets.destination);
        let content_hash = crate::ops::skill_content_hash(fs, ctx, targets.destination)?;
        let home_doc = home_document.as_mut().unwrap_or(&mut document);
        let copies = home_doc
            .entry("copies".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let serde_json::Value::Object(copies) = copies {
            copies.insert(
                deployment_id.clone(),
                serde_json::json!({
                    "deployment_id": deployment_id,
                    "name": req.skill.0,
                    "path": targets.destination,
                    "scope": crate::ops::scope_label(&req.scope),
                    "destination": "universal",
                    "slot": "universal",
                    "project_path": match &req.scope {
                        RootScope::Global => None,
                        RootScope::Project(p) => Some(p.0.clone()),
                    },
                    "content_hash": content_hash,
                    "disabled": false,
                }),
            );
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

    Ok(linked)
}

/// `Copy`: stages `files` under [`journal_root`]'s [`FsJournal`], then
/// swaps the staged folder into `<universal_root>/<skill>`. The journal's
/// own crash from a previous install is already swept by this call's
/// `MutationSession::begin`, so this only opens it, never reconciles it a
/// second time.
fn install_copy(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    universal_root: &Path,
    skill: &SkillName,
    files: &[InstallFile],
) -> Result<(), CoreError> {
    let fs = rt.ports.fs.clone();
    // R1 fallout: this journal is always rooted under the scope home (see
    // the doc on `journal_root`'s only call site in `MutationSession::begin`),
    // never under the op's own target root - so for a project-scope install,
    // `<home>/.agents` was never brought up by the caller's own
    // `ensure_dir_all(targets.universal_root)`, which only reaches the
    // *project's* `.agents`.
    ensure_journal_root(rt, guard, fs.as_ref())?;
    let journal_root = journal_root(&rt.scope.home.lexical);
    let journal = FsJournal::new(journal_root, fs.clone());

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

/// Symlinks `link_path` (`<scope>/.claude/skills/<skill>`) to `destination`,
/// unless `.claude/skills` is already a whole-directory link into the shared
/// root (every skill is already visible through it) - mirrors the guard in
/// `ops::set_claude_code_switch` - or `link_path` itself already exists
/// (R3): `cli_args_and_cwd` passes `--agent claude-code` for a `SkillsSh`
/// install that requests the Claude Code harness, so the CLI already created
/// this exact link before this call ever runs; treating that as done rather
/// than an `EEXIST` failure mirrors the desktop's
/// `skill_add::maybe_claude_code_symlink`.
fn link_claude_code(
    rt: &Runtime,
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    destination: &Path,
    link_path: &Path,
) -> Result<(), CoreError> {
    let claude_skills_dir = link_path
        .parent()
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidRequest, "link path has no parent"))?;
    if fs
        .symlink_metadata(claude_skills_dir)
        .is_ok_and(|f| f.kind == FileKind::Symlink)
    {
        return Ok(());
    }
    crate::ops::ensure_dir_all(rt, session, fs, claude_skills_dir)?;
    if fs.symlink_metadata(link_path).is_ok() {
        return Ok(());
    }
    let scoped_target = ports::confine(&rt.scope, fs, destination)?;
    let scoped_link = ports::confine(&rt.scope, fs, link_path)?;
    fs.symlink(&session.guard, &scoped_target, &scoped_link)
        .map_err(|e| CoreError::io(link_path, e))
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
