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
//! `Map::remove`) keep an untouched key's position stable across a write.
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
use crate::ports::{
    self, ExclusiveGuard, FileKind, MutationSession, OpContext, PlanStatus, ProcessSpec, Runtime,
    ScopeFs,
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
/// through [`registry::write_registry_document_locked`] under the caller's
/// already-held exclusive lease - `write_version` is bumped there, not by
/// this op, and every key besides the handful `install` itself touches
/// round-trips untouched.
fn write_registry_document(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    fs: &dyn ScopeFs,
    scope_root: &Path,
    document: serde_json::Map<String, serde_json::Value>,
) -> Result<(), CoreError> {
    let _ = &rt.scope;
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

/// Normalizes a `trust_identity` for lookup/storage: trims whitespace,
/// drops a trailing `.git`, lowercases. Mirrors the desktop's
/// `skill_trust_policy::normalize_confirmation_identity`, minus the
/// multi-line/empty rejection (an empty identity is never gated by this
/// op).
fn normalize_identity(identity: &str) -> String {
    identity
        .trim()
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

/// The `npx` package an [`InstallMethod`] shells out to, or `None` for
/// `Copy` (which never calls `npx`). A plain lookup rather than an
/// `unreachable!` arm, so a caller that mismatches method and code path
/// gets a typed error instead of a panic.
fn cli_package(method: InstallMethod) -> Option<&'static str> {
    match method {
        InstallMethod::Dotagents => Some("@sentry/dotagents"),
        InstallMethod::SkillsSh => Some("skills"),
        InstallMethod::Copy => None,
    }
}

/// Percent-encode `/` and `%` so a project path can sit in a single
/// deployment-id slot - mirrors the desktop's
/// `skill_deployment::encode_id_path`.
fn encode_id_path_segment(path: &str) -> String {
    path.replace('%', "%25").replace('/', "%2F")
}

/// Builds the same `dep:v1/{scope}/{slot}/{destination}/{name}/{project}/
/// {lexical-entry}` id the desktop's `skill_deployment::deployment_id` does,
/// for the `Copy` `copies` entry this op writes into the registry - `slot`
/// and `destination` are always `universal` for a `Copy` install, since this
/// op only ever writes the shared universal root (see the module doc,
/// "Linking").
fn copy_deployment_id(scope: &RootScope, skill: &SkillName, destination: &Path) -> String {
    let scope_label = crate::ops::scope_label(scope);
    let project = match scope {
        RootScope::Global => "-".to_string(),
        RootScope::Project(project) => encode_id_path_segment(&project.0.to_string_lossy()),
    };
    format!(
        "dep:v1/{scope_label}/universal/universal/{}/{project}/{}",
        skill.0,
        encode_id_path_segment(&destination.to_string_lossy())
    )
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
        if !req.trust_confirmed && !is_trusted(&document, identity) {
            return Ok(InstallOutcome::NeedsTrust {
                identity: identity.clone(),
            });
        }
        if req.trust_confirmed {
            record_trusted(&mut document, identity);
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
    let inverse = serde_json::json!({
        "op": "remove_install",
        "path": destination,
        "claude_link": claude_link_path,
    });
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
    match install_and_link(rt, ctx, &mut session, fs, req, &targets, document) {
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

/// The write-and-link step every `install` call shares, once its journal
/// row is already recorded: creates `targets.universal_root`, writes
/// `req.method`'s bytes, links Claude Code when requested, and writes the
/// registry document back - any failure here bubbles up so `install` can
/// mark the row `Failed` (F9).
fn install_and_link(
    rt: &Runtime,
    ctx: &OpContext,
    session: &mut MutationSession,
    fs: &dyn ScopeFs,
    req: &InstallRequest,
    targets: &InstallTargets,
    mut document: serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<AgentId>, CoreError> {
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
        let copies = document
            .entry("copies".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
        if let serde_json::Value::Object(copies) = copies {
            copies.insert(
                req.skill.0.clone(),
                serde_json::json!({
                    "deployment_id": copy_deployment_id(&req.scope, &req.skill, targets.destination),
                    "name": req.skill.0,
                    "path": targets.destination,
                    "scope": crate::ops::scope_label(&req.scope),
                    "destination": "universal",
                    "slot": "universal",
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
    write_registry_document(rt, &session.guard, fs, targets.root, document)?;

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
    let journal_root = journal_root(&rt.scope.home.lexical);
    let scoped_journal_root = ports::confine(&rt.scope, fs.as_ref(), &journal_root)?;
    fs.create_dir_all(guard, &scoped_journal_root)
        .map_err(|e| CoreError::io(&journal_root, e))?;
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

/// Builds the argv `install_via_cli` hands the spawner, and the process cwd
/// to run it in - ported from the desktop's own builders: skills.sh from
/// `skill_install_plan.rs`'s `skills_sh_universal_add_args` (`npx skills add
/// <source> --yes --global | --cwd <p> [--skill <n>] --agent universal
/// [--agent claude-code]`, per the request's chosen harnesses; the process
/// cwd itself is never set - the target scope travels through `--global`/
/// `--cwd` instead), and dotagents from `skill_add.rs`'s `add_via_dotagents`
/// (`npx -y @sentry/dotagents [--project] add <source> [--name <n>]`, this
/// time with the process cwd itself set to the project path for a project
/// scope).
fn cli_args_and_cwd(
    method: InstallMethod,
    source: &str,
    skill: &SkillName,
    scope: &RootScope,
    harnesses: &[AgentId],
) -> (Vec<String>, Option<PathBuf>) {
    match method {
        InstallMethod::SkillsSh => {
            let mut args = vec![
                "skills".to_string(),
                "add".to_string(),
                source.to_string(),
                "--yes".to_string(),
            ];
            match scope {
                RootScope::Global => args.push("--global".to_string()),
                RootScope::Project(project) => {
                    args.push("--cwd".to_string());
                    args.push(project.0.to_string_lossy().into_owned());
                }
            }
            args.push("--skill".to_string());
            args.push(skill.0.clone());
            args.push("--agent".to_string());
            args.push("universal".to_string());
            if harnesses.iter().any(|h| h.as_str() == AgentId::CLAUDE_CODE) {
                args.push("--agent".to_string());
                args.push("claude-code".to_string());
            }
            (args, None)
        }
        InstallMethod::Dotagents => {
            let mut args = vec!["-y".to_string(), "@sentry/dotagents".to_string()];
            let cwd = match scope {
                RootScope::Global => None,
                RootScope::Project(project) => {
                    args.push("--project".to_string());
                    Some(project.0.clone())
                }
            };
            args.push("add".to_string());
            args.push(source.to_string());
            args.push("--name".to_string());
            args.push(skill.0.clone());
            (args, cwd)
        }
        InstallMethod::Copy => (Vec::new(), None),
    }
}

/// `Dotagents`/`SkillsSh`: runs `req.method`'s argv (see
/// [`cli_args_and_cwd`]) through the process-spawner port and checks the
/// destination now exists. The CLI writes its own files directly - see the
/// module doc for why this op does not stage-and-swap them.
fn install_via_cli(
    rt: &Runtime,
    ctx: &OpContext,
    req: &InstallRequest,
    destination: &Path,
) -> Result<(), CoreError> {
    let Some(source) = req.source.as_deref() else {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "a dotagents/skills.sh install needs a source",
        ));
    };
    if cli_package(req.method).is_none() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "install_via_cli is never called for Copy",
        ));
    }
    let spawner = rt.ports.spawner.as_ref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::Unsupported,
            "this host build has no process spawner; dotagents/skills.sh installs are not available",
        )
    })?;
    let (args, cwd) = cli_args_and_cwd(req.method, source, &req.skill, &req.scope, &req.harnesses);
    let spec = ProcessSpec {
        program: "npx".to_string(),
        args,
        cwd,
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

/// Symlinks `link_path` (`<scope>/.claude/skills/<skill>`) to `destination`,
/// unless `.claude/skills` is already a whole-directory link into the shared
/// root (every skill is already visible through it) - mirrors the guard in
/// `ops::set_claude_code_switch`.
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
    let scoped_target = ports::confine(&rt.scope, fs, destination)?;
    let scoped_link = ports::confine(&rt.scope, fs, link_path)?;
    fs.symlink(&session.guard, &scoped_target, &scoped_link)
        .map_err(|e| CoreError::io(link_path, e))
}
