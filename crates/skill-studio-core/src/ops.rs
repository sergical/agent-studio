//! Operations and the result envelope.
//!
//! Each operation is a plain function over a [`Runtime`] and an
//! [`OpContext`]. Adapters wrap the result in a [`ResultEnvelope`] with
//! [`ResultEnvelope::from_result`]; the exit status is derived, never chosen.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tiktoken_rs::CoreBPE;

use crate::dto::{
    CapabilitiesRequest, Completeness, DeploymentDto, Diagnosis, DriftState, EventDto,
    FrontmatterRepairPreview, InstalledSkillDto, Inventory, Issue, IssueKind, ListEventsRequest,
    NextAction, Observation, PluginSourceDto, RepairApplyMode, RepairApplyRequest, RepairOutcome,
    RepairPreviewRequest, RestoreOutcome, RestoreRequest, ScanRequest, Severity, Timing,
};
use crate::error::{CoreError, ErrorCode, ErrorEntry};
use crate::events::EventFilter;
use crate::frontmatter;
use crate::frontmatter_repair::propose_colon_scalar_repair;
use crate::harness::{
    Capabilities, CapabilityReport, DisabledBy, HarnessFacts, HarnessObserved, RootRole,
    ScopeLevel, Support, ToolAvailability,
};
use crate::identity::{
    AgentId, BackingRelationship, CorrelationId, DeploymentId, DeploymentMutability, EventId,
    Fingerprint, LifecycleOwnerKind, OwnerId, ProjectRef, RootKind, RootRef, RootScope,
    SkillDestination, SkillName, SourceKind, MOVE_ASIDE_DIR_NAME, PARKED_ROOT_RELATIVE,
};
use crate::lock_file;
use crate::ownership;
use crate::ports::{
    acquire_shared, DirEntryFacts, FileKind, HistoryAccess, OpContext, Runtime, ScopeFs,
    ScopedReads,
};
use crate::scope::{EffectiveScope, NormalizedScope};
use crate::SCHEMA_VERSION;

/// Default number of history rows returned by `list_events`.
pub const DEFAULT_EVENT_LIMIT: u32 = 200;

/// Largest `SKILL.md` `scan` will read. A file over this size is reported as
/// an unreadable root entry rather than truncated.
pub const SKILL_MD_MAX_BYTES: u64 = 2 * 1024 * 1024;

/// Largest number of files `content_fingerprint` will hash per deployment.
const MAX_FOLDER_FILES: usize = 2_000;
/// Largest total bytes `content_fingerprint` will read per deployment.
const MAX_FOLDER_BYTES: u64 = 64 * 1024 * 1024;
/// Manifest filenames a plugin cache walk looks for, in priority order,
/// per the agent-plugins.org manifest convention.
const PLUGIN_MANIFEST_CANDIDATES: &[&str] = &[
    ".claude-plugin/plugin.json",
    ".codex-plugin/plugin.json",
    ".cursor-plugin/plugin.json",
    "plugin.json",
];
/// Depth `find_plugin_roots` walks below a plugin cache root before giving
/// up on finding a manifest.
const PLUGIN_CACHE_MAX_DEPTH: u8 = 3;

/// Name of an operation, as it appears in the envelope and in MCP tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    /// Phase 1: inventory.
    Scan,
    /// Phase 1: inventory plus issues.
    Diagnose,
    /// Phase 1: harness facts.
    Capabilities,
    /// Phase 2: propose a frontmatter fix.
    PreviewFrontmatterRepair,
    /// Phase 2: apply a proposed fix.
    ApplyFrontmatterRepair,
    /// Phase 2: read history.
    ListEvents,
    /// Phase 2: revert one event.
    RestoreEvent,
}

/// Outcome status of one call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OpStatus {
    /// Full result.
    Ok,
    /// Result with gaps; `errors` explains them.
    Partial,
    /// No result.
    Error,
}

/// A result type that knows whether it is complete.
///
/// Invariant: a `Partial` outcome maps to exit status 4 even though no
/// error was raised.
pub trait Outcome {
    /// Status of this value.
    fn status(&self) -> OpStatus {
        OpStatus::Ok
    }

    /// True when a complete result still reports problems the user must
    /// look at (`diagnose` with issues). Maps to exit status `1`.
    fn found_issues(&self) -> bool {
        false
    }

    /// The history event this outcome created, when it created one.
    ///
    /// Every surface builds its envelope through `from_result`, so an
    /// outcome that answers here fills the envelope's `event_id` the same
    /// way for the CLI, the MCP server and the desktop. An outcome that
    /// writes nothing keeps the default.
    fn event_id(&self) -> Option<EventId> {
        None
    }
}

impl Outcome for Inventory {
    fn status(&self) -> OpStatus {
        match self.completeness {
            Completeness::Complete => OpStatus::Ok,
            Completeness::Partial => OpStatus::Partial,
        }
    }
}

impl Outcome for Diagnosis {
    fn status(&self) -> OpStatus {
        self.inventory.status()
    }

    fn found_issues(&self) -> bool {
        self.issues
            .iter()
            .any(|i| i.severity >= crate::dto::Severity::Warning)
    }
}

impl Outcome for Capabilities {}
impl Outcome for FrontmatterRepairPreview {}
impl Outcome for RepairOutcome {
    fn event_id(&self) -> Option<EventId> {
        match self {
            // `AlreadyApplied` wrote nothing, so it records no event.
            RepairOutcome::Applied { event_id, .. } => Some(event_id.clone()),
            RepairOutcome::AlreadyApplied { .. } => None,
        }
    }
}
impl Outcome for Vec<EventDto> {}
impl Outcome for RestoreOutcome {
    fn event_id(&self) -> Option<EventId> {
        // The restore event, not the event it reverted: `event_id` names
        // what this call created.
        Some(self.restore_event_id.clone())
    }
}

/// The envelope every surface returns.
///
/// Invariant: `exit_status` is a pure function of `status` and `errors`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ResultEnvelope<T> {
    /// Wire contract version.
    pub schema_version: u32,
    /// Operation name.
    pub operation: Operation,
    /// Scope the call ran in.
    pub scope: EffectiveScope,
    /// Outcome.
    pub status: OpStatus,
    /// Result data, `None` on error.
    pub data: Option<T>,
    /// Errors, empty on `Ok`.
    pub errors: Vec<ErrorEntry>,
    /// Request correlation id.
    pub correlation_id: CorrelationId,
    /// History event created by the call, when one was.
    pub event_id: Option<EventId>,
}

impl<T: Outcome> ResultEnvelope<T> {
    /// Wraps an operation result.
    pub fn from_result(
        operation: Operation,
        scope: &NormalizedScope,
        correlation_id: CorrelationId,
        result: Result<T, CoreError>,
    ) -> Self {
        let (status, data, errors) = match result {
            Ok(value) => {
                let status = value.status();
                let errors = if status == OpStatus::Partial {
                    vec![ErrorEntry {
                        code: ErrorCode::Incomplete,
                        message: "some roots were not read; see observations".into(),
                        path: None,
                    }]
                } else {
                    Vec::new()
                };
                (status, Some(value), errors)
            }
            Err(err) => (OpStatus::Error, None, vec![err.sanitized(scope)]),
        };
        let event_id = data.as_ref().and_then(Outcome::event_id);
        ResultEnvelope {
            schema_version: SCHEMA_VERSION,
            operation,
            scope: scope.effective(),
            status,
            data,
            errors,
            correlation_id,
            event_id,
        }
    }
}

impl<T: Outcome> ResultEnvelope<T> {
    /// Process exit status: `0` ok, `1` ok with issues, `4` partial, else
    /// the first error's code. Partial wins over issues.
    pub fn exit_status(&self) -> i32 {
        match self.status {
            OpStatus::Ok if self.data.as_ref().is_some_and(|d| d.found_issues()) => 1,
            OpStatus::Ok => 0,
            OpStatus::Partial => ErrorCode::Incomplete.exit_status(),
            OpStatus::Error => self
                .errors
                .first()
                .map(|e| e.code.exit_status())
                .unwrap_or(1),
        }
    }
}

/// Reads every root in scope and returns the inventory.
///
/// Preconditions: shared lease within `read_timeout`. Never creates the
/// history database, caches, or watchers. Roots that cannot be read inside
/// the budget are reported in `observations` and make the result `Partial`.
///
/// Walks the catalog's [`RootRole::Own`], [`RootRole::Universal`], and
/// [`RootRole::Legacy`] roots, plus the parked root
/// ([`crate::identity::RootKind::Parked`]) and, inside every one of those, a
/// [`MOVE_ASIDE_DIR_NAME`] holding directory whose children are reported
/// disabled ([`DisabledBy::StudioMoved`]). [`RootRole::PluginCache`] roots
/// are walked separately by [`plugin_scan_targets`]. `RootRole::CrossHarness`
/// stays unwalked: it names a root another harness owns that this harness
/// also reads, and walking it again under this harness would double-report
/// the same directory.
pub fn scan(rt: &Runtime, ctx: &OpContext, req: &ScanRequest) -> Result<Inventory, CoreError> {
    ctx.checkpoint()?;
    let _guard = acquire_shared(rt.ports.leases.as_ref(), &rt.scope)?;
    scan_inner(rt, ctx, req)
}

/// The scan walk itself, without acquiring a lease.
///
/// [`scan`] takes the shared lease and calls this. [`crate::ports::MutationSession::begin`]
/// already holds the exclusive lease when it needs a fresh inventory, and an
/// advisory file lock does not nest within one process, so it calls this
/// directly instead of `scan`.
pub(crate) fn scan_inner(
    rt: &Runtime,
    ctx: &OpContext,
    req: &ScanRequest,
) -> Result<Inventory, CoreError> {
    let start = rt.ports.clock.monotonic();
    let budget = rt.scope.raw.read_timeout();
    let fs = rt.ports.fs.as_ref();
    let home = &rt.scope.home.lexical;

    let disable_sources = DisableSources::read(fs, home);

    // Full ownership classification needs the dotagents and skills.sh
    // ledgers for every scope this scan covers (the home's `.agents` plus
    // each tracked project's), and the Skill-Studio-owned copy/fork
    // registry, which lives at the home only. Matches the desktop's
    // `load_ownership_ledgers`/`read_fork_registry_or_default`
    // (`skill_ownership.rs`/`skill_fork_registry.rs`).
    let mut scope_ledgers: HashMap<RootScope, ownership::ScopeLedgers> = HashMap::new();
    scope_ledgers.insert(
        RootScope::Global,
        ownership::read_scope_ledgers(fs, &home.join(".agents")),
    );
    for project in &rt.scope.projects {
        scope_ledgers.insert(
            RootScope::Project(ProjectRef(project.lexical.clone())),
            ownership::read_scope_ledgers(fs, &project.lexical.join(".agents")),
        );
    }
    let home_registry = ownership::read_home_registry(fs, home);

    let sc = ScanCtx {
        rt,
        ctx,
        req,
        fs,
        home,
        disable_sources: &disable_sources,
        scope_ledgers: &scope_ledgers,
        home_registry: &home_registry,
        start,
        budget,
    };
    let mut accum = ScanAccum {
        skills: BTreeMap::new(),
        observations: Vec::new(),
        completeness: Completeness::Complete,
        // Deployment id -> canonical directory, filled in by
        // `process_entries` and consumed by
        // `propagate_verified_linked_owners` once every root has been
        // walked.
        resolved_paths: HashMap::new(),
    };

    // Global roots (and their plugin caches) go first so a scan that runs
    // out of read budget on a home with many projects still reaches every
    // home-scoped root before spending the budget on project roots. See
    // the module doc for the exact order.
    let (global_targets, project_targets): (Vec<_>, Vec<_>) = scan_targets(rt)
        .into_iter()
        .partition(|target| matches!(target.scope, RootScope::Global));
    let (global_plugin_targets, project_plugin_targets): (Vec<_>, Vec<_>) = plugin_scan_targets(rt)
        .into_iter()
        .partition(|target| matches!(target.scope, RootScope::Global));

    for target in global_targets {
        scan_one_target(&sc, target, &mut accum)?;
    }
    for target in global_plugin_targets {
        scan_one_plugin_target(&sc, target, &mut accum)?;
    }
    for target in project_targets {
        scan_one_target(&sc, target, &mut accum)?;
    }
    for target in project_plugin_targets {
        scan_one_plugin_target(&sc, target, &mut accum)?;
    }

    let ScanAccum {
        mut skills,
        observations,
        completeness,
        resolved_paths,
    } = accum;

    // Every root has been walked, so every canonical universal deployment
    // any link could point to now has a `resolved_paths` entry: assign
    // verified links their canonical owner.
    for skill in skills.values_mut() {
        propagate_verified_linked_owners(skill, &resolved_paths);
    }

    // A universal skill Claude Code has no per-skill link for is one that
    // reader has disabled: Claude Code only ever reads a universal skill
    // through an explicit `~/.claude/skills/<name>` link, never the shared
    // root directly (`reads_universal_root: Support::No` in `harness.rs`).
    let claude_skills_dir = home.join(".claude").join("skills");
    for skill in skills.values_mut() {
        for deployment in &mut skill.deployments {
            let is_global_universal = matches!(deployment.root.kind, RootKind::Universal)
                && matches!(deployment.root.scope, RootScope::Global);
            if is_global_universal
                && fs
                    .symlink_metadata(&claude_skills_dir.join(&skill.name.0))
                    .is_err()
            {
                deployment
                    .disabled_readers
                    .push(AgentId::from(AgentId::CLAUDE_CODE));
            }
        }
    }

    let skills: Vec<InstalledSkillDto> = skills.into_values().collect();

    let mut timings = Vec::new();
    if req.timings {
        let elapsed = rt.ports.clock.monotonic().saturating_sub(start);
        timings.push(Timing {
            phase: "scan".to_string(),
            elapsed_ms: elapsed.as_millis() as u64,
        });
    }

    Ok(Inventory {
        skills,
        projects: rt
            .scope
            .projects
            .iter()
            .map(|p| p.lexical.clone())
            .collect(),
        completeness,
        observations,
        timings,
    })
}

/// Everything [`scan_one_target`] and [`scan_one_plugin_target`] need that
/// stays the same across every target in one [`scan_inner`] call.
struct ScanCtx<'a> {
    rt: &'a Runtime,
    ctx: &'a OpContext,
    req: &'a ScanRequest,
    fs: &'a dyn ScopeFs,
    home: &'a Path,
    disable_sources: &'a DisableSources,
    scope_ledgers: &'a HashMap<RootScope, ownership::ScopeLedgers>,
    home_registry: &'a ownership::HomeRegistry,
    start: Duration,
    budget: Duration,
}

/// The `scan_inner` accumulators every target folds into, in target order.
struct ScanAccum {
    skills: BTreeMap<String, InstalledSkillDto>,
    observations: Vec<Observation>,
    completeness: Completeness,
    /// Deployment id -> canonical directory, filled in by `process_entries`
    /// and consumed by `propagate_verified_linked_owners` once every root
    /// has been walked.
    resolved_paths: HashMap<DeploymentId, PathBuf>,
}

/// Reads one [`ScanTarget`] (and its [`MOVE_ASIDE_DIR_NAME`] holding
/// directory) into `accum`, or records a budget/read-error observation.
/// The body of `scan_inner`'s former `for target in scan_targets(rt)` loop.
fn scan_one_target(
    sc: &ScanCtx,
    target: ScanTarget,
    accum: &mut ScanAccum,
) -> Result<(), CoreError> {
    sc.ctx.checkpoint()?;
    if sc.rt.ports.clock.monotonic().saturating_sub(sc.start) > sc.budget {
        accum.completeness = Completeness::Partial;
        accum.observations.push(Observation {
            root: RootRef::new(target.scope.clone(), target.kind.clone()).ok(),
            message: "read budget exceeded before this root could be scanned".to_string(),
        });
        return Ok(());
    }

    // A root whose lexical path is itself a symlink (e.g. `~/.claude/
    // skills -> ../.agents/skills`) shares every deployment under it
    // through that one link, not per skill.
    let whole_dir_link = matches!(
        sc.fs.symlink_metadata(&target.path).map(|m| m.kind),
        Ok(FileKind::Symlink)
    );

    match read_root_entries(sc.fs, &target.path) {
        Ok(names) => process_entries(
            &EntryContext {
                fs: sc.fs,
                ctx: sc.ctx,
                scope: &sc.rt.scope,
                home: sc.home,
                disable_sources: sc.disable_sources,
                scope_ledgers: sc.scope_ledgers,
                home_registry: sc.home_registry,
                target: &target,
                base_dir: &target.path,
                whole_dir_link,
                forced_disabled_by: None,
            },
            &names,
            sc.req,
            &mut accum.skills,
            &mut accum.observations,
            &mut accum.completeness,
            &mut accum.resolved_paths,
        )?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            accum.completeness = Completeness::Partial;
            accum.observations.push(Observation {
                root: RootRef::new(target.scope.clone(), target.kind.clone()).ok(),
                message: format!("could not read root: {e}"),
            });
        }
    }

    // Skills Skill Studio moved aside stay deployments (so the UI can
    // still show and un-park them), just disabled.
    let move_aside_dir = target.path.join(MOVE_ASIDE_DIR_NAME);
    if let Ok(names) = read_root_entries(sc.fs, &move_aside_dir) {
        process_entries(
            &EntryContext {
                fs: sc.fs,
                ctx: sc.ctx,
                scope: &sc.rt.scope,
                home: sc.home,
                disable_sources: sc.disable_sources,
                scope_ledgers: sc.scope_ledgers,
                home_registry: sc.home_registry,
                target: &target,
                base_dir: &move_aside_dir,
                whole_dir_link,
                forced_disabled_by: Some(DisabledBy::StudioMoved),
            },
            &names,
            sc.req,
            &mut accum.skills,
            &mut accum.observations,
            &mut accum.completeness,
            &mut accum.resolved_paths,
        )?;
    }
    Ok(())
}

/// Reads one [`PluginCacheTarget`] into `accum`, or records a budget
/// observation. The body of `scan_inner`'s former
/// `for target in plugin_scan_targets(rt)` loop.
fn scan_one_plugin_target(
    sc: &ScanCtx,
    target: PluginCacheTarget,
    accum: &mut ScanAccum,
) -> Result<(), CoreError> {
    sc.ctx.checkpoint()?;
    if sc.rt.ports.clock.monotonic().saturating_sub(sc.start) > sc.budget {
        accum.completeness = Completeness::Partial;
        accum.observations.push(Observation {
            root: RootRef::new(
                target.scope.clone(),
                RootKind::PluginCache(target.harness.clone()),
            )
            .ok(),
            message: "read budget exceeded before this root could be scanned".to_string(),
        });
        return Ok(());
    }
    for plugin_skill in enumerate_plugin_skills(sc.fs, &target) {
        if !sc.req.skills.is_empty() && !sc.req.skills.iter().any(|s| s.0 == plugin_skill.name) {
            continue;
        }
        match read_skill_md(sc.fs, sc.ctx, &plugin_skill.skill_dir, &plugin_skill.name)? {
            SkillMdRead::Found {
                description,
                violations,
                truncated,
                facts,
            } => {
                if truncated {
                    if let Some(observation) = truncated_skill_md_observation(
                        target.scope.clone(),
                        RootKind::PluginCache(target.harness.clone()),
                        &plugin_skill.name,
                    ) {
                        accum.observations.push(observation);
                    }
                }
                let content_fingerprint = Some(content_fingerprint(sc.fs, &plugin_skill.skill_dir));
                // Matches the desktop's plugin-cache loop: `is_symlink`
                // is checked, but a plugin skill is never resolved
                // through it, so the link facts otherwise stay at their
                // defaults.
                let is_symlink = matches!(
                    sc.fs
                        .symlink_metadata(&plugin_skill.skill_dir)
                        .map(|f| f.kind),
                    Ok(FileKind::Symlink)
                );
                let scope_label = scope_label(&target.scope);
                let project_label = project_label(&target.scope);
                let plugin_source = PluginSourceDto {
                    enabled: claude_plugin_enabled(
                        sc.disable_sources,
                        &target.harness,
                        &plugin_skill.source,
                    ),
                    ..plugin_skill.source.clone()
                };
                let deployment = DeploymentDto {
                    id: deployment_id(
                        &plugin_skill.name,
                        scope_label,
                        SkillDestination::PerHarness,
                        &harness_slot(&target.harness),
                        project_label.as_deref(),
                        &plugin_skill.skill_dir,
                    ),
                    root: RootRef::new(
                        target.scope.clone(),
                        RootKind::PluginCache(target.harness.clone()),
                    )?,
                    harness: Some(target.harness.clone()),
                    path: plugin_skill.skill_dir.clone(),
                    destination: SkillDestination::PerHarness,
                    // Matches `id_for_candidate`: a plugin cache entry
                    // is never the universal root and is never linked,
                    // so it falls to the same `Independent` branch as
                    // any other plain per-harness directory.
                    backing: BackingRelationship::Independent,
                    mutability: DeploymentMutability::ReadOnly,
                    link_target: None,
                    shared_via_whole_dir_link: false,
                    is_symlink,
                    resolved_path: None,
                    symlink_is_broken: false,
                    symlink_error: None,
                    owner_kind: LifecycleOwnerKind::Plugin,
                    owner_id: None,
                    content_fingerprint,
                    disabled_by: None,
                    disabled_readers: Vec::new(),
                    spec_violations: violations,
                    plugin: Some(plugin_source),
                    frontmatter: facts.frontmatter,
                    frontmatter_fields: facts.frontmatter_fields,
                    has_spec: facts.has_spec,
                    folder_bytes: facts.folder_bytes,
                    file_count: facts.file_count,
                    skill_md_tokens: facts.skill_md_tokens,
                    description_tokens: facts.description_tokens,
                    content_hash: facts.content_hash,
                    modified_at: facts.modified_at,
                    folder_truncated: facts.folder_truncated,
                    // A plugin cache root is never walked through the
                    // `.skill-studio-disabled/` move-aside directory.
                    in_git_repo: in_git_repo(sc.fs, &sc.rt.scope, &plugin_skill.skill_dir),
                    studio_disabled: false,
                    source_kind: SourceKind::Plugin,
                };
                insert_deployment(
                    &mut accum.skills,
                    &plugin_skill.name,
                    description,
                    deployment,
                );
            }
            SkillMdRead::Unreadable(message) => {
                accum.completeness = Completeness::Partial;
                accum.observations.push(Observation {
                    root: RootRef::new(
                        target.scope.clone(),
                        RootKind::PluginCache(target.harness.clone()),
                    )
                    .ok(),
                    message,
                });
            }
            SkillMdRead::NotASkill => {}
        }
    }
    Ok(())
}

/// Lists a root directory's visible skill-shaped entries (dirs and
/// symlinks), sorted for a deterministic scan order. Dot-prefixed entries
/// (including [`MOVE_ASIDE_DIR_NAME`] itself) are never a skill; the caller
/// walks that holding directory separately.
fn read_root_entries(fs: &dyn ScopeFs, dir: &Path) -> std::io::Result<Vec<DirEntryFacts>> {
    let entries = fs.read_dir(dir)?;
    let mut names: Vec<_> = entries
        .into_iter()
        .filter(|e| matches!(e.kind, FileKind::Dir | FileKind::Symlink))
        .filter(|e| !e.name.starts_with('.'))
        .collect();
    names.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(names)
}

/// Everything [`process_entries`] needs that stays the same across every
/// entry in one call: which root, which reader, which sources of truth.
struct EntryContext<'a> {
    fs: &'a dyn ScopeFs,
    ctx: &'a OpContext,
    scope: &'a NormalizedScope,
    home: &'a Path,
    disable_sources: &'a DisableSources,
    /// Dotagents and skills.sh ledgers, one entry per scope this scan
    /// covers. Read once in [`scan_inner`]; [`classify_owner`] looks up the
    /// entry matching `target.scope`.
    scope_ledgers: &'a HashMap<RootScope, ownership::ScopeLedgers>,
    /// The copy/fork buckets of `~/.agents/skill-studio.json`, read once at
    /// the scope home.
    home_registry: &'a ownership::HomeRegistry,
    target: &'a ScanTarget,
    /// Where the entries physically live: `target.path` normally, or its
    /// [`MOVE_ASIDE_DIR_NAME`] child when walking moved-aside skills.
    base_dir: &'a Path,
    whole_dir_link: bool,
    /// `Some(StudioMoved)` while walking a move-aside directory; every
    /// deployment found there is disabled regardless of any other switch.
    forced_disabled_by: Option<DisabledBy>,
}

/// Builds one deployment per entry and files it under its skill name, or
/// records a partial-scan observation for one that could not be read.
///
/// `resolved_paths` collects each built deployment's canonical directory
/// (its own path for a plain canonical entry, or the symlink/whole-dir-link
/// target's canonical path for a linked one), keyed by the deployment's id.
/// [`propagate_verified_linked_owners`] uses it after every root has been
/// walked to match a linked deployment back to its canonical counterpart,
/// mirroring the desktop's `resolved_path`-keyed match in
/// `skill_assembly.rs`.
fn process_entries(
    cx: &EntryContext,
    names: &[DirEntryFacts],
    req: &ScanRequest,
    skills: &mut BTreeMap<String, InstalledSkillDto>,
    observations: &mut Vec<Observation>,
    completeness: &mut Completeness,
    resolved_paths: &mut HashMap<DeploymentId, PathBuf>,
) -> Result<(), CoreError> {
    for entry in names {
        cx.ctx.checkpoint()?;
        if !req.skills.is_empty() && !req.skills.iter().any(|s| s.0 == entry.name) {
            continue;
        }

        let skill_dir = cx.base_dir.join(&entry.name);
        let is_link = matches!(entry.kind, FileKind::Symlink);
        // Three states: a target that resolves; a target that is missing,
        // which is a broken link; and a target that fails to resolve for
        // any other reason - a permission denial, a symlink loop - which
        // is NOT broken and carries the reason instead. Collapsing the
        // last two would label a link the user cannot read as one they
        // must repair.
        let (canonical, broken_link, symlink_error) =
            match is_link.then(|| cx.fs.canonicalize(&skill_dir)) {
                None => (None, false, None),
                Some(Ok(target)) => (Some(target), false, None),
                Some(Err(err)) if err.kind() == std::io::ErrorKind::NotFound => (None, true, None),
                Some(Err(err)) => (None, false, Some(err.to_string())),
            };
        // A per-skill symlink whose target does not resolve still names a
        // deployment (so a broken link is visible and repairable), just one
        // with no bytes to fingerprint. Both unresolved states qualify: an
        // unreadable link has no bytes either.
        let unresolved_link = is_link && canonical.is_none();
        // The canonicalized target when the link resolves, or the raw
        // `read_link` value joined onto the link's own parent (not
        // filesystem-canonicalized) when it doesn't, so a broken link's
        // target is still comparable and still recognizable as pointing
        // into the universal root.
        let link_target = if is_link {
            match &canonical {
                Some(target) => Some(target.clone()),
                None => cx.fs.read_link(&skill_dir).ok().map(|raw| {
                    if raw.is_absolute() {
                        raw
                    } else {
                        skill_dir.parent().unwrap_or(Path::new("")).join(raw)
                    }
                }),
            }
        } else {
            None
        };

        let (description, violations, content_fingerprint, facts) = if unresolved_link {
            (None, Vec::new(), None, Box::new(ContentFacts::default()))
        } else {
            match read_skill_md(cx.fs, cx.ctx, &skill_dir, &entry.name)? {
                SkillMdRead::Found {
                    description,
                    violations,
                    truncated,
                    facts,
                } => {
                    if truncated {
                        if let Some(observation) = truncated_skill_md_observation(
                            cx.target.scope.clone(),
                            cx.target.kind.clone(),
                            &entry.name,
                        ) {
                            observations.push(observation);
                        }
                    }
                    (
                        description,
                        violations,
                        Some(content_fingerprint(cx.fs, &skill_dir)),
                        facts,
                    )
                }
                SkillMdRead::NotASkill => continue,
                SkillMdRead::Unreadable(message) => {
                    *completeness = Completeness::Partial;
                    observations.push(Observation {
                        root: RootRef::new(cx.target.scope.clone(), cx.target.kind.clone()).ok(),
                        message,
                    });
                    continue;
                }
            }
        };

        // Matches the desktop's `id_for_candidate` (`skill_deployment.rs`):
        // a root that is itself universal is always Canonical; otherwise a
        // per-skill symlink into the universal root, or a whole-directory
        // link (the root itself is a symlink), promotes the deployment to
        // the Universal destination as a link back to that canonical entry;
        // everything else is an Independent per-harness deployment.
        let root_is_universal = matches!(cx.target.kind, RootKind::Universal | RootKind::Parked);
        let linked = !root_is_universal
            && (cx.whole_dir_link
                || (is_link
                    && link_target
                        .as_deref()
                        .is_some_and(path_is_under_universal_skills)));
        let (destination, backing) = if root_is_universal {
            (SkillDestination::Universal, BackingRelationship::Canonical)
        } else if linked {
            (SkillDestination::Universal, BackingRelationship::LinkedTo)
        } else {
            (
                SkillDestination::PerHarness,
                BackingRelationship::Independent,
            )
        };

        let scope_label = scope_label(&cx.target.scope);
        let project_label = project_label(&cx.target.scope);
        let id = deployment_id(
            &entry.name,
            scope_label,
            destination,
            &harness_slot_for_kind(&cx.target.kind),
            project_label.as_deref(),
            &skill_dir,
        );
        // A canonical entry's own directory is its resolved path; a linked
        // one resolves to wherever it points (`None` for a broken link,
        // which never has anything to propagate from or to).
        let resolved_path = if is_link {
            canonical.clone()
        } else {
            Some(
                cx.fs
                    .canonicalize(&skill_dir)
                    .unwrap_or_else(|_| skill_dir.clone()),
            )
        };
        if let Some(resolved_path) = resolved_path {
            resolved_paths.insert(id.clone(), resolved_path);
        }
        // The DTO's `resolved_path` (not the internal `resolved_path` above,
        // which serves owner propagation and differs on purpose): for a
        // link it's the canonicalized target; otherwise it's the entry's
        // own canonicalized path, but only when that differs from the entry
        // itself, and with no fallback to the un-canonicalized path on
        // error.
        let dto_resolved_path = if is_link {
            canonical.clone()
        } else {
            cx.fs
                .canonicalize(&skill_dir)
                .ok()
                .filter(|c| c != &skill_dir)
        };
        let in_git_repo = in_git_repo(cx.fs, cx.scope, &skill_dir);
        // Matches the variant, not merely "forced": the field means the
        // deployment sits in a `.skill-studio-disabled/` holding directory,
        // and a future forced reason must not claim that.
        let studio_disabled = cx.forced_disabled_by == Some(DisabledBy::StudioMoved);

        let (owner_kind, owner_id) = classify_owner(&OwnerClassifyContext {
            home: cx.home,
            kind: &cx.target.kind,
            scope: &cx.target.scope,
            scope_ledgers: cx.scope_ledgers,
            home_registry: cx.home_registry,
            skill_name: &entry.name,
            skill_dir: &skill_dir,
            destination,
            is_link,
            link_target: link_target.as_deref(),
            id: &id,
            content_fingerprint: content_fingerprint.as_ref(),
            disabled: studio_disabled,
            in_git_repo,
        });
        let mutability = if owner_kind.is_mutable() {
            DeploymentMutability::Mutable
        } else {
            DeploymentMutability::ReadOnly
        };

        let disabled_by = cx.forced_disabled_by.or_else(|| {
            native_disabled_by(cx.disable_sources, &cx.target.kind, &skill_dir, &entry.name)
        });

        let source_kind = source_kind_from_owner(owner_kind);
        let deployment = DeploymentDto {
            id,
            root: match RootRef::new(cx.target.scope.clone(), cx.target.kind.clone()) {
                Ok(root) => root,
                Err(_) => continue,
            },
            harness: cx.target.harness.clone(),
            path: skill_dir.clone(),
            destination,
            backing,
            mutability,
            link_target,
            shared_via_whole_dir_link: cx.whole_dir_link,
            is_symlink: is_link,
            resolved_path: dto_resolved_path,
            symlink_is_broken: broken_link,
            symlink_error,
            owner_kind,
            owner_id,
            content_fingerprint,
            disabled_by,
            disabled_readers: Vec::new(),
            spec_violations: violations,
            plugin: None,
            frontmatter: facts.frontmatter,
            frontmatter_fields: facts.frontmatter_fields,
            has_spec: facts.has_spec,
            folder_bytes: facts.folder_bytes,
            file_count: facts.file_count,
            skill_md_tokens: facts.skill_md_tokens,
            description_tokens: facts.description_tokens,
            content_hash: facts.content_hash,
            modified_at: facts.modified_at,
            folder_truncated: facts.folder_truncated,
            in_git_repo,
            studio_disabled,
            source_kind,
        };

        insert_deployment(skills, &entry.name, description, deployment);
    }
    Ok(())
}

/// Assigns a verified linked deployment (a per-skill symlink or
/// whole-directory link into the universal root) the same owner as the
/// canonical universal deployment it points to, and forces it read-only:
/// lifecycle actions must target the canonical deployment, not one of its
/// links. Scoped to one skill's deployments at a time since
/// [`Inventory::skills`] already groups them by name.
///
/// A link matches its canonical counterpart when they share a root scope
/// and the link's resolved directory (from `resolved_paths`) equals the
/// canonical entry's own directory; a link with no unambiguous match (none,
/// or more than one) is left as classified.
fn propagate_verified_linked_owners(
    skill: &mut InstalledSkillDto,
    resolved_paths: &HashMap<DeploymentId, PathBuf>,
) {
    let canonical_owners: Vec<(RootScope, PathBuf, LifecycleOwnerKind, Option<OwnerId>)> = skill
        .deployments
        .iter()
        .filter(|d| {
            d.destination == SkillDestination::Universal
                && matches!(d.backing, BackingRelationship::Canonical)
                && d.owner_kind.is_mutable()
        })
        .filter_map(|d| {
            resolved_paths.get(&d.id).map(|rp| {
                (
                    d.root.scope.clone(),
                    rp.clone(),
                    d.owner_kind,
                    d.owner_id.clone(),
                )
            })
        })
        .collect();

    for deployment in &mut skill.deployments {
        if !matches!(deployment.backing, BackingRelationship::LinkedTo)
            || deployment.destination != SkillDestination::Universal
        {
            continue;
        }
        let link_shape_is_verified =
            deployment.link_target.is_some() || deployment.shared_via_whole_dir_link;
        if !link_shape_is_verified {
            continue;
        }
        let Some(resolved_path) = resolved_paths.get(&deployment.id) else {
            continue;
        };
        let mut matches = canonical_owners
            .iter()
            .filter(|(scope, rp, _, _)| *scope == deployment.root.scope && rp == resolved_path);
        let Some((_, _, owner_kind, owner_id)) = matches.next() else {
            continue;
        };
        if matches.next().is_some() {
            continue;
        }
        deployment.owner_kind = *owner_kind;
        deployment.owner_id = owner_id.clone();
        deployment.mutability = DeploymentMutability::ReadOnly;
    }
}

fn insert_deployment(
    skills: &mut BTreeMap<String, InstalledSkillDto>,
    name: &str,
    description: Option<String>,
    deployment: DeploymentDto,
) {
    let skill = skills
        .entry(name.to_string())
        .or_insert_with(|| InstalledSkillDto {
            name: SkillName(name.to_string()),
            description: None,
            deployments: Vec::new(),
        });
    if skill.description.is_none() {
        skill.description = description;
    }
    skill.deployments.push(deployment);
}

/// Outcome of reading and validating `<skill_dir>/SKILL.md`.
///
/// The [`Result`] `read_skill_md` returns this in is cancellation only: `Err`
/// means [`compute_content_facts`]'s folder walk was cancelled mid-walk, and
/// must abort the whole scan rather than be folded into a partial-scan
/// observation (a cancelled walk's facts are not a real, if incomplete,
/// read - they are no read at all).
enum SkillMdRead {
    /// A real skill. `truncated` is `true` when `SKILL.md` was over
    /// [`SKILL_MD_MAX_BYTES`] and read only up to the cap rather than
    /// dropped; the caller notes that as an [`Observation`] (it never makes
    /// the scan `Partial`, since the skill and its bytes up to the cap were
    /// still read successfully).
    Found {
        description: Option<String>,
        violations: Vec<String>,
        truncated: bool,
        facts: Box<ContentFacts>,
    },
    /// No `SKILL.md`: not a skill directory. The caller skips the entry
    /// silently.
    NotASkill,
    /// `SKILL.md` exists but could not be read; the message is a
    /// partial-scan observation for the caller to report.
    Unreadable(String),
}

/// Reads and validates `<skill_dir>/SKILL.md`. See [`SkillMdRead`] for what
/// each outcome means to the caller.
fn read_skill_md(
    fs: &dyn ScopeFs,
    ctx: &OpContext,
    skill_dir: &Path,
    name: &str,
) -> Result<SkillMdRead, CoreError> {
    let skill_md = skill_dir.join("SKILL.md");
    let (bytes, truncated) = match fs.read_prefix(&skill_md, SKILL_MD_MAX_BYTES) {
        Ok(result) => result,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(SkillMdRead::NotASkill),
        Err(e) => {
            return Ok(SkillMdRead::Unreadable(format!(
                "could not read {}: {e}",
                skill_md.display()
            )))
        }
    };
    let content = String::from_utf8_lossy(&bytes);
    let parsed = frontmatter::parse_frontmatter(&content);
    let violations = frontmatter::validate_skill(name, &parsed, content.lines().count());
    let description = parsed.as_frontmatter().and_then(|f| f.description.clone());
    let facts = compute_content_facts(fs, ctx, skill_dir, &bytes, truncated, &parsed)?;
    Ok(SkillMdRead::Found {
        description,
        violations,
        truncated,
        facts: Box::new(facts),
    })
}

/// Message prefix a truncated-`SKILL.md` observation always carries.
///
/// The observation still names a root ([`Observation::root`]) so a caller
/// can locate it, but it reports a file that was read successfully up to
/// the cap, not a root `diagnose` should call [`crate::dto::IssueKind::RootUnreadable`]:
/// [`derive_issues`] recognizes this prefix and skips it there.
const TRUNCATED_SKILL_MD_PREFIX: &str = "skill_md_truncated:";

/// Builds the observation noting a `SKILL.md` read that hit
/// [`SKILL_MD_MAX_BYTES`] and was truncated rather than dropped.
fn truncated_skill_md_observation(
    scope: RootScope,
    kind: RootKind,
    name: &str,
) -> Option<Observation> {
    RootRef::new(scope, kind).ok().map(|root| Observation {
        root: Some(root),
        message: format!(
            "{TRUNCATED_SKILL_MD_PREFIX} {name}'s SKILL.md is over the {SKILL_MD_MAX_BYTES} \
             byte read cap; only the first {SKILL_MD_MAX_BYTES} bytes were read"
        ),
    })
}

/// One root, resolved to a concrete filesystem path, that `scan` walks with
/// [`process_entries`]. Built from the catalog's [`RootRole::Own`],
/// [`RootRole::Universal`], and [`RootRole::Legacy`] roots, plus the parked
/// root, which the catalog does not carry since it belongs to Skill Studio
/// rather than to any one harness.
struct ScanTarget {
    scope: RootScope,
    kind: RootKind,
    path: PathBuf,
    harness: Option<AgentId>,
}

fn scan_targets(rt: &Runtime) -> Vec<ScanTarget> {
    let mut seen: HashSet<(RootScope, RootKind, PathBuf)> = HashSet::new();
    let mut targets = Vec::new();
    for facts in &rt.ports.catalog.facts {
        for root_spec in &facts.roots {
            let (kind, harness) = match root_spec.role {
                RootRole::Own => (RootKind::Harness(facts.id.clone()), Some(facts.id.clone())),
                RootRole::Universal => (RootKind::Universal, None),
                RootRole::Legacy => (RootKind::Legacy(facts.id.clone()), Some(facts.id.clone())),
                RootRole::PluginCache | RootRole::CrossHarness => continue,
            };
            let mut push_target = |scope: RootScope, path: PathBuf| {
                let key = (scope.clone(), kind.clone(), path.clone());
                if seen.insert(key) {
                    targets.push(ScanTarget {
                        scope,
                        kind: kind.clone(),
                        path,
                        harness: harness.clone(),
                    });
                }
            };
            match root_spec.level {
                ScopeLevel::Global => {
                    push_target(
                        RootScope::Global,
                        rt.scope.home.lexical.join(&root_spec.relative_path),
                    );
                }
                ScopeLevel::Project => {
                    for project in &rt.scope.projects {
                        push_target(
                            RootScope::Project(ProjectRef(project.lexical.clone())),
                            project.lexical.join(&root_spec.relative_path),
                        );
                    }
                }
            }
        }
    }
    targets.push(ScanTarget {
        scope: RootScope::Global,
        kind: RootKind::Parked,
        path: rt.scope.home.lexical.join(PARKED_ROOT_RELATIVE),
        harness: None,
    });
    targets
}

/// One harness's plugin cache root, resolved to a concrete filesystem path.
struct PluginCacheTarget {
    scope: RootScope,
    harness: AgentId,
    path: PathBuf,
}

fn plugin_scan_targets(rt: &Runtime) -> Vec<PluginCacheTarget> {
    let mut targets = Vec::new();
    for facts in &rt.ports.catalog.facts {
        for root_spec in &facts.roots {
            if root_spec.role != RootRole::PluginCache {
                continue;
            }
            match root_spec.level {
                ScopeLevel::Global => targets.push(PluginCacheTarget {
                    scope: RootScope::Global,
                    harness: facts.id.clone(),
                    path: rt.scope.home.lexical.join(&root_spec.relative_path),
                }),
                ScopeLevel::Project => {
                    for project in &rt.scope.projects {
                        targets.push(PluginCacheTarget {
                            scope: RootScope::Project(ProjectRef(project.lexical.clone())),
                            harness: facts.id.clone(),
                            path: project.lexical.join(&root_spec.relative_path),
                        });
                    }
                }
            }
        }
    }
    targets
}

/// One skill directory found inside a plugin's `skills/` subdirectory.
struct PluginSkillDir {
    name: String,
    skill_dir: PathBuf,
    source: PluginSourceDto,
}

/// Parses a `plugin.json`-shaped manifest leniently: only `name` is
/// required; a missing, unreadable, or malformed manifest yields `None`
/// rather than failing the walk. Reports "no plugin here" instead of
/// guessing a name from the directory.
fn read_plugin_manifest(fs: &dyn ScopeFs, plugin_dir: &Path) -> Option<Option<String>> {
    for candidate in PLUGIN_MANIFEST_CANDIDATES {
        let manifest_path = plugin_dir.join(candidate);
        let Ok(bytes) = fs.read_capped(&manifest_path, SKILL_MD_MAX_BYTES) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            return Some(None);
        };
        if value.get("name").and_then(|v| v.as_str()).is_some() {
            let version = value
                .get("version")
                .and_then(|v| v.as_str())
                .map(str::to_string);
            return Some(version);
        }
        return Some(None);
    }
    None
}

/// Walks a plugin cache tree up to [`PLUGIN_CACHE_MAX_DEPTH`] levels for
/// plugin roots (a directory holding one of [`PLUGIN_MANIFEST_CANDIDATES`]
/// directly), then lists each plugin's `skills/<name>` directories.
/// `marketplace`/`plugin`/`version` come from the plugin root's path
/// relative to `target.path`, per the agent-plugins.org
/// `<cache>/<marketplace>/<plugin>/<version>/` layout.
fn enumerate_plugin_skills(fs: &dyn ScopeFs, target: &PluginCacheTarget) -> Vec<PluginSkillDir> {
    let mut plugin_roots = Vec::new();
    walk_for_plugin_roots(fs, &target.path, PLUGIN_CACHE_MAX_DEPTH, &mut plugin_roots);

    let mut out = Vec::new();
    for plugin_root in plugin_roots {
        let rel = plugin_root
            .strip_prefix(&target.path)
            .unwrap_or(&plugin_root);
        let components: Vec<String> = rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        let source = PluginSourceDto {
            marketplace: components.first().cloned().unwrap_or_default(),
            plugin: components.get(1).cloned().unwrap_or_default(),
            version: components.get(2).cloned(),
            // Filled in by `scan_one_plugin_target` from `DisableSources`
            // once the target's harness is known.
            enabled: None,
        };
        let skills_dir = plugin_root.join("skills");
        let Ok(entries) = fs.read_dir(&skills_dir) else {
            continue;
        };
        for entry in entries {
            if !matches!(entry.kind, FileKind::Dir) {
                continue;
            }
            let skill_dir = skills_dir.join(&entry.name);
            if fs.read_capped(&skill_dir.join("SKILL.md"), 1).is_err()
                && fs
                    .read_capped(&skill_dir.join("SKILL.md"), SKILL_MD_MAX_BYTES)
                    .is_err()
            {
                continue;
            }
            out.push(PluginSkillDir {
                name: entry.name.clone(),
                skill_dir,
                source: source.clone(),
            });
        }
    }
    out
}

fn walk_for_plugin_roots(
    fs: &dyn ScopeFs,
    dir: &Path,
    depth_remaining: u8,
    found: &mut Vec<PathBuf>,
) {
    let Ok(entries) = fs.read_dir(dir) else {
        return;
    };
    for entry in entries {
        if !matches!(entry.kind, FileKind::Dir) {
            continue;
        }
        let path = dir.join(&entry.name);
        if read_plugin_manifest(fs, &path).is_some() {
            found.push(path);
            continue;
        }
        if depth_remaining > 0 {
            walk_for_plugin_roots(fs, &path, depth_remaining - 1, found);
        }
    }
}

/// Codex and OpenCode's own per-skill disable switches, read once per
/// `scan` call (they are global config files, not per-root).
struct DisableSources {
    /// Canonical `SKILL.md` paths Codex's `[[skills.config]] enabled =
    /// false` rows name. Mirrors `codex_skill_config.rs`
    /// `read_disabled_skill_md_paths`.
    codex_disabled_skill_md: Vec<PathBuf>,
    /// Skill names `permission.skill.<name> = "deny"` denies in
    /// `opencode.json`. Mirrors `opencode_skill_permission.rs`
    /// `read_denied_patterns`; the core matches names exactly and does not
    /// implement that function's `*` glob support.
    opencode_denied_skills: Vec<String>,
    /// Claude Code `settings.json` `enabledPlugins["<plugin>@<marketplace>"]`,
    /// keyed by that same `<plugin>@<marketplace>` id.
    claude_enabled_plugins: HashMap<String, bool>,
}

impl DisableSources {
    fn read(fs: &dyn ScopeFs, home: &Path) -> Self {
        DisableSources {
            codex_disabled_skill_md: read_codex_disabled_skill_md_paths(fs, home),
            opencode_denied_skills: read_opencode_denied_skills(fs, home),
            claude_enabled_plugins: read_claude_enabled_plugins(fs, home),
        }
    }
}

fn read_codex_disabled_skill_md_paths(fs: &dyn ScopeFs, home: &Path) -> Vec<PathBuf> {
    let path = home.join(".codex").join("config.toml");
    let Ok(bytes) = fs.read_capped(&path, SKILL_MD_MAX_BYTES) else {
        return Vec::new();
    };
    let Ok(text) = String::from_utf8(bytes) else {
        return Vec::new();
    };
    // `toml::Value::from_str` parses one value literal, not a document;
    // `toml::Table` is the document-level parser.
    let Ok(table) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    let value = toml::Value::Table(table);
    value
        .get("skills")
        .and_then(|s| s.get("config"))
        .and_then(|c| c.as_array())
        .into_iter()
        .flatten()
        .filter(|row| row.get("enabled").and_then(toml::Value::as_bool) == Some(false))
        .filter_map(|row| row.get("path").and_then(toml::Value::as_str))
        .map(PathBuf::from)
        .collect()
}

fn read_opencode_denied_skills(fs: &dyn ScopeFs, home: &Path) -> Vec<String> {
    let path = home.join(".config").join("opencode").join("opencode.json");
    let Ok(bytes) = fs.read_capped(&path, SKILL_MD_MAX_BYTES) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Vec::new();
    };
    let Some(skill) = value
        .get("permission")
        .and_then(|p| p.get("skill"))
        .and_then(|s| s.as_object())
    else {
        return Vec::new();
    };
    skill
        .iter()
        .filter(|(_, v)| v.as_str() == Some("deny"))
        .map(|(name, _)| name.clone())
        .collect()
}

/// Reads Claude Code's global `enabledPlugins` map, keyed
/// `<plugin>@<marketplace>`. A missing or malformed `settings.json` yields
/// an empty map, so every lookup falls back to `None`.
fn read_claude_enabled_plugins(fs: &dyn ScopeFs, home: &Path) -> HashMap<String, bool> {
    let path = home.join(".claude").join("settings.json");
    let Ok(bytes) = fs.read_capped(&path, SKILL_MD_MAX_BYTES) else {
        return HashMap::new();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return HashMap::new();
    };
    let Some(enabled_plugins) = value.get("enabledPlugins").and_then(|v| v.as_object()) else {
        return HashMap::new();
    };
    enabled_plugins
        .iter()
        .filter_map(|(id, v)| v.as_bool().map(|b| (id.clone(), b)))
        .collect()
}

/// Claude Code's `enabledPlugins` state for one plugin deployment, or `None`
/// for any other harness (Codex has no such record).
fn claude_plugin_enabled(
    sources: &DisableSources,
    harness: &AgentId,
    source: &PluginSourceDto,
) -> Option<bool> {
    if harness.as_str() != AgentId::CLAUDE_CODE {
        return None;
    }
    let id = format!("{}@{}", source.plugin, source.marketplace);
    sources.claude_enabled_plugins.get(&id).copied()
}

fn native_disabled_by(
    sources: &DisableSources,
    kind: &RootKind,
    skill_dir: &Path,
    name: &str,
) -> Option<DisabledBy> {
    let skill_md = skill_dir.join("SKILL.md");
    match kind {
        RootKind::Harness(id) if id.as_str() == AgentId::CODEX => sources
            .codex_disabled_skill_md
            .iter()
            .any(|p| p == &skill_md)
            .then_some(DisabledBy::CodexConfig),
        RootKind::Harness(id) | RootKind::Legacy(id) if id.as_str() == AgentId::OPEN_CODE => {
            sources
                .opencode_denied_skills
                .iter()
                .any(|n| n == name)
                .then_some(DisabledBy::OpencodePermission)
        }
        _ => None,
    }
}

/// True when `path`'s components contain a contiguous `[".agents",
/// "skills"]` window, i.e. it names something inside the universal root.
/// Mirrors the desktop's `path_is_under_universal_skills`
/// (`skill_deployment.rs`); works on a raw (unresolved) symlink target the
/// same way the desktop does, since a link written as `../../.agents/
/// skills/<name>` carries the window regardless of the leading `..`s.
fn path_is_under_universal_skills(path: &Path) -> bool {
    let components: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    components
        .windows(2)
        .any(|w| w[0] == ".agents" && w[1] == "skills")
}

/// Everything [`classify_owner`] needs to decide one deployment's owner.
/// Grouped into one struct because the four-ledger precedence needs facts
/// from three different points in [`process_entries`] (the entry itself,
/// its freshly derived id, and its freshly computed content fingerprint),
/// not just the root it was found in.
struct OwnerClassifyContext<'a> {
    /// Scope home, for the fork registry's default path
    /// (`<home>/.agents/skills/<name>`).
    home: &'a Path,
    kind: &'a RootKind,
    scope: &'a RootScope,
    scope_ledgers: &'a HashMap<RootScope, ownership::ScopeLedgers>,
    home_registry: &'a ownership::HomeRegistry,
    skill_name: &'a str,
    skill_dir: &'a Path,
    destination: SkillDestination,
    /// True for a per-skill symlink; false for a canonical directory or a
    /// whole-directory link (whose entries are canonical directories one
    /// level down).
    is_link: bool,
    /// Where a per-skill symlink points, when it is one. A link into the
    /// universal root makes the owner ambiguous - see `classify_owner`.
    link_target: Option<&'a Path>,
    id: &'a DeploymentId,
    content_fingerprint: Option<&'a Fingerprint>,
    /// True when this entry sits in a `.skill-studio-disabled/` holding
    /// directory (`studio_disabled`).
    disabled: bool,
    in_git_repo: bool,
}

/// Classifies which ledger owns a deployment's lifecycle, per the precedence
/// in `docs/spec-core-primitives.md` section 13.4: `Plugin` (root
/// is a plugin cache) > `Fork` (detached from its ledger via "Fork", so it
/// must win even over a matching ledger entry) > `Copy` (an exact,
/// content-matched deployment the Copy installer recorded) > `Ambiguous`
/// (both the dotagents ledger and the skills.sh lock file claim the same
/// name) > `Dotagents` / `WildcardDotagents` (named vs. wildcard
/// `agents.toml` row) > `SkillsSh` > `InRepo` (a `.git` ancestor) >
/// `Manual`.
///
/// The dotagents and skills.sh ledgers only ever apply to a deployment
/// rooted directly in the universal or parked root: a per-harness
/// deployment, or a linked one whose root isn't itself universal, falls
/// straight to `InRepo`/`Manual` here and is corrected afterward by
/// [`propagate_verified_linked_owners`] once its canonical counterpart's
/// owner is known.
fn classify_owner(cx: &OwnerClassifyContext) -> (LifecycleOwnerKind, Option<OwnerId>) {
    if matches!(cx.kind, RootKind::PluginCache(_)) {
        return (LifecycleOwnerKind::Plugin, None);
    }

    // Fork: only ever the global scope's canonical Universal deployment
    // (`apply_skill_snapshot_overlays`, `skill_refresh.rs`), and it wins
    // over everything below since forking detaches a skill from whatever
    // ledger it came from.
    if matches!(cx.scope, RootScope::Global)
        && cx.destination == SkillDestination::Universal
        && !cx.is_link
    {
        if let Some(record) = cx.home_registry.forks.get(cx.skill_name) {
            let expected_dir = if record.skill_dir.as_os_str().is_empty() {
                cx.home.join(".agents").join("skills").join(cx.skill_name)
            } else {
                record.skill_dir.clone()
            };
            let id_matches =
                record.deployment_id.is_empty() || record.deployment_id == cx.id.as_str();
            if cx.skill_dir == expected_dir && id_matches {
                let owner = owner_id("global", None, cx.skill_name);
                return (LifecycleOwnerKind::Fork, Some(owner));
            }
        }
    }

    if let Some(record) = cx.home_registry.copies.get(cx.id.as_str()) {
        let scope_matches = matches!(
            (cx.scope, record.scope.as_str()),
            (RootScope::Global, "global") | (RootScope::Project(_), "project")
        );
        let destination_matches = record.destination
            == match cx.destination {
                SkillDestination::Universal => "universal",
                SkillDestination::PerHarness => "per-harness",
            };
        let project_matches = match cx.scope {
            RootScope::Global => record.project_path.is_none(),
            RootScope::Project(project) => {
                record.project_path.as_deref() == Some(project.0.to_string_lossy().as_ref())
            }
        };
        let content_matches = !record.content_hash.is_empty()
            && cx
                .content_fingerprint
                .is_some_and(|fp| fp.bare_hex() == record.content_hash);
        if scope_matches
            && destination_matches
            && project_matches
            && content_matches
            && record.name == cx.skill_name
            && record.path == cx.skill_dir
            && record.disabled == cx.disabled
        {
            return (LifecycleOwnerKind::Copy, None);
        }
    }

    let root_is_universal = matches!(cx.kind, RootKind::Universal | RootKind::Parked);
    if !root_is_universal {
        return if cx.in_git_repo {
            (LifecycleOwnerKind::InRepo, None)
        } else {
            (LifecycleOwnerKind::Manual, None)
        };
    }

    // `scope_ledgers` always has an entry for both the home scope and every
    // tracked project (`scan_inner`), even when neither ledger file exists,
    // so an empty ledger and a missing one behave the same: no dotagents or
    // skills.sh entry, fall through to `InRepo`/`Manual` below.
    let ledger = cx
        .scope_ledgers
        .get(cx.scope)
        .expect("scan_inner populates a ledger for every scope it walks");

    let dotagents_entry = ledger.dotagents.iter().find(|d| d.name == cx.skill_name);
    let skills_sh_entry = lock_file::is_skill_installed(&ledger.lock, cx.skill_name);

    if dotagents_entry.is_some() && skills_sh_entry {
        return (LifecycleOwnerKind::Ambiguous, None);
    }

    if let Some(entry) = dotagents_entry {
        let owner = owner_id(
            scope_label(cx.scope),
            project_label(cx.scope).as_deref(),
            cx.skill_name,
        );
        return if !entry.has_manifest_row {
            (LifecycleOwnerKind::WildcardDotagents, Some(owner))
        } else {
            (LifecycleOwnerKind::Dotagents, Some(owner))
        };
    }

    if skills_sh_entry {
        let owner = owner_id(
            scope_label(cx.scope),
            project_label(cx.scope).as_deref(),
            cx.skill_name,
        );
        return (LifecycleOwnerKind::SkillsSh, Some(owner));
    }

    // Two carve-outs applied before falling back, both of which narrow the
    // owner rather than widen it. Owner kind gates repair, so skipping them
    // would permit owner-wide actions that neither carve-out's ambiguity
    // should allow.

    // A universal root sitting beside an `agents.toml` or `agents.lock` that
    // names no row for this skill: the dotagents install owns the root, so
    // the skill reads as dotagents-managed, but no row says who owns it.
    if matches!(cx.kind, RootKind::Universal) && ledger.has_dotagents_files {
        return (LifecycleOwnerKind::Ambiguous, None);
    }

    // A per-skill symlink into the universal root: the bytes belong to the
    // universal deployment, whose own ledger entry may say otherwise, so
    // this end of the link claims no owner.
    if cx.is_link && cx.link_target.is_some_and(resolves_into_dotagents) {
        return (LifecycleOwnerKind::Ambiguous, None);
    }

    if cx.in_git_repo {
        (LifecycleOwnerKind::InRepo, None)
    } else {
        (LifecycleOwnerKind::Manual, None)
    }
}

fn scope_label(scope: &RootScope) -> &'static str {
    match scope {
        RootScope::Global => "global",
        RootScope::Project(_) => "project",
    }
}

fn project_label(scope: &RootScope) -> Option<String> {
    match scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.to_string_lossy().into_owned()),
    }
}

/// The `dep:v1` id's harness/universal path segment for a root, matching the
/// desktop's `harness_slot` (`skill_deployment.rs`): every harness's own
/// wire id, except OpenCode, whose slot is the un-hyphenated CLI name
/// `opencode`; `universal` for the shared and parked roots.
fn harness_slot(id: &AgentId) -> String {
    if id.as_str() == AgentId::OPEN_CODE {
        "opencode".to_string()
    } else {
        id.as_str().to_string()
    }
}

fn harness_slot_for_kind(kind: &RootKind) -> String {
    match kind {
        RootKind::Harness(id) | RootKind::Legacy(id) | RootKind::PluginCache(id) => {
            harness_slot(id)
        }
        RootKind::Universal | RootKind::Parked => "universal".to_string(),
    }
}

/// Percent-encodes `%` and `/` so a path segment can sit inside a `/`-joined
/// id without its own separators colliding with the id's. Matches the
/// desktop's `encode_id_path` (`skill_deployment.rs`) byte for byte.
fn encode_id_path(path: &str) -> String {
    path.replace('%', "%25").replace('/', "%2F")
}

/// Derives a deployment id in the desktop's exact wire format: `dep:v1/
/// {scope}/{slot}/{destination}/{name}/{project}/{lexical-entry}`. Matches
/// the desktop's `deployment_id` (`skill_deployment.rs`) byte for byte, given
/// the same inputs.
fn deployment_id(
    name: &str,
    scope_label: &str,
    destination: SkillDestination,
    slot: &str,
    project_path: Option<&str>,
    lexical_entry: &Path,
) -> DeploymentId {
    let project = match project_path {
        Some(path) if !path.is_empty() => encode_id_path(path),
        _ => "-".to_string(),
    };
    let destination_label = match destination {
        SkillDestination::Universal => "universal",
        SkillDestination::PerHarness => "per-harness",
    };
    let raw = format!(
        "{}{scope_label}/{slot}/{destination_label}/{name}/{project}/{}",
        DeploymentId::PREFIX,
        encode_id_path(&lexical_entry.to_string_lossy())
    );
    DeploymentId::parse(&raw).expect("well-formed deployment id")
}

/// Derives the owner id for a skills.sh-owned deployment. Matches the
/// desktop's `owner_id_for` (`skill_ownership.rs`) byte for byte.
fn owner_id(scope_label: &str, project_path: Option<&str>, skill_name: &str) -> OwnerId {
    let raw = match (scope_label, project_path) {
        ("global", _) => format!("owner:v1/global/{skill_name}"),
        (_, Some(path)) if !path.is_empty() => {
            format!("owner:v1/project/{}/{skill_name}", encode_id_path(path))
        }
        _ => format!("owner:v1/project/-/{skill_name}"),
    };
    OwnerId::parse(&raw).expect("well-formed owner id")
}

/// Content fingerprint over a deployment's whole directory tree: sha256 over
/// the sorted `(relative path, bytes)` pairs, each length-framed as `u64 LE
/// len(rel_path) || rel_path bytes || u64 LE file_len || file bytes`, capped
/// at [`MAX_FOLDER_FILES`] files and [`MAX_FOLDER_BYTES`] total bytes. The
/// truncation edge case (a file so large only part of it fits the
/// remaining byte budget) degrades to "read nothing further", since
/// [`ScopeFs::read_capped`] has no partial-read primitive.
fn content_fingerprint(fs: &dyn ScopeFs, dir: &Path) -> Fingerprint {
    let mut files = Vec::new();
    let mut total_bytes = 0u64;
    let mut file_count = 0usize;
    walk_content_files(fs, dir, dir, &mut files, &mut total_bytes, &mut file_count);
    files.sort_by(|a, b| a.0.cmp(&b.0));

    let mut buf = Vec::new();
    let mut remaining = MAX_FOLDER_BYTES;
    for (rel_path, abs_path, len) in &files {
        let rel_bytes = rel_path.to_string_lossy().into_owned().into_bytes();
        buf.extend_from_slice(&(rel_bytes.len() as u64).to_le_bytes());
        buf.extend_from_slice(&rel_bytes);
        buf.extend_from_slice(&len.to_le_bytes());
        if remaining == 0 {
            continue;
        }
        match fs.read_capped(abs_path, remaining.min(*len)) {
            Ok(bytes) => {
                remaining = remaining.saturating_sub(bytes.len() as u64);
                buf.extend_from_slice(&bytes);
            }
            Err(_) => remaining = 0,
        }
    }
    Fingerprint::of_bytes(&buf)
}

/// Recursively collects `(relative path, absolute path, len)` for every
/// regular file under `dir`, stopping once [`MAX_FOLDER_FILES`] or
/// [`MAX_FOLDER_BYTES`] is reached. Never follows a symlinked directory or
/// reads a symlinked file.
fn walk_content_files(
    fs: &dyn ScopeFs,
    root: &Path,
    dir: &Path,
    files: &mut Vec<(PathBuf, PathBuf, u64)>,
    total_bytes: &mut u64,
    file_count: &mut usize,
) {
    if *file_count >= MAX_FOLDER_FILES || *total_bytes >= MAX_FOLDER_BYTES {
        return;
    }
    let Ok(entries) = fs.read_dir(dir) else {
        return;
    };
    let mut names: Vec<_> = entries;
    names.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in names {
        if *file_count >= MAX_FOLDER_FILES || *total_bytes >= MAX_FOLDER_BYTES {
            return;
        }
        let path = dir.join(&entry.name);
        match entry.kind {
            FileKind::Dir => walk_content_files(fs, root, &path, files, total_bytes, file_count),
            FileKind::File => {
                let Ok(meta) = fs.symlink_metadata(&path) else {
                    continue;
                };
                let remaining = MAX_FOLDER_BYTES.saturating_sub(*total_bytes);
                if meta.len > remaining {
                    *total_bytes = MAX_FOLDER_BYTES;
                    continue;
                }
                *total_bytes += meta.len;
                *file_count += 1;
                if let Ok(rel) = path.strip_prefix(root) {
                    files.push((rel.to_path_buf(), path.clone(), meta.len));
                }
            }
            FileKind::Symlink | FileKind::Other => {}
        }
    }
}

/// The embedded cl100k_base vocab is loaded once per process.
static TOKENIZER: OnceLock<Option<CoreBPE>> = OnceLock::new();

fn tokenizer() -> Option<&'static CoreBPE> {
    TOKENIZER
        .get_or_init(|| tiktoken_rs::cl100k_base().ok())
        .as_ref()
}

/// Token count of `text`, cl100k_base. `None` tokenizer (the embedded vocab
/// failed to build, which should never happen) yields 0.
fn count_tokens(text: &str, tokenizer: Option<&CoreBPE>) -> u32 {
    tokenizer
        .map(|bpe| bpe.encode_with_special_tokens(text).len() as u32)
        .unwrap_or(0)
}

/// True when `skill_dir` follows a symlink to an existing file or directory.
/// `ScopeFs` has no "metadata that follows links" primitive, so a symlink is
/// resolved by hand: `canonicalize` then `symlink_metadata` on the target.
fn exists_following_links(fs: &dyn ScopeFs, path: &Path) -> bool {
    match fs.symlink_metadata(path) {
        Ok(meta) if meta.kind == FileKind::Symlink => fs.canonicalize(path).is_ok(),
        Ok(_) => true,
        Err(_) => false,
    }
}

/// True when `path` follows a symlink to an existing directory.
fn is_dir_following_links(fs: &dyn ScopeFs, path: &Path) -> bool {
    match fs.symlink_metadata(path) {
        Ok(meta) if meta.kind == FileKind::Dir => true,
        Ok(meta) if meta.kind == FileKind::Symlink => fs
            .canonicalize(path)
            .ok()
            .and_then(|target| fs.symlink_metadata(&target).ok())
            .is_some_and(|m| m.kind == FileKind::Dir),
        _ => false,
    }
}

/// A skill "ships specs" (the getsentry/skillet pattern) when it has a
/// `spec.md` file or an `evals/` subdirectory alongside `SKILL.md`.
fn has_spec(fs: &dyn ScopeFs, skill_dir: &Path) -> bool {
    exists_following_links(fs, &skill_dir.join("spec.md"))
        || is_dir_following_links(fs, &skill_dir.join("evals"))
}

/// A regular or symlinked-to-a-file entry found while walking a skill folder
/// for content facts, queued for hashing once the whole folder has been
/// walked and its entries sorted. Holds only the path and size, never the
/// file's bytes, so the walk's memory use doesn't grow with folder size.
struct HashableFile {
    rel_path: PathBuf,
    abs_path: PathBuf,
    len: u64,
}

/// Accumulated facts from walking a skill folder for [`DeploymentDto`]'s
/// content facts. Distinct from [`content_fingerprint`]/[`walk_content_files`]
/// (the older fingerprint scheme): this walk also counts a symlinked file
/// toward `file_count` (without ever hashing it) and tracks the newest
/// mtime.
#[derive(Default)]
struct FactsWalk {
    hashable: Vec<HashableFile>,
    total_bytes: u64,
    file_count: u32,
    newest: Option<DateTime<Utc>>,
    truncated: bool,
}

/// Walks `dir` recursively into `walk`, gathering byte/file counts and the
/// newest mtime, stopping once [`MAX_FOLDER_FILES`]/`max_bytes` is reached.
/// Never follows a symlinked directory; a symlinked file counts toward
/// `file_count` but is never opened, hashed, or sized into `total_bytes`.
/// Unreadable entries are skipped rather than failing the whole walk. A
/// per-directory-entry [`OpContext::checkpoint`] means a cancellation here
/// must fail the whole walk (and so the caller's digest) rather than return
/// whatever partial `walk` was accumulated so far, since a short walk would
/// silently produce a plausible-but-wrong content hash.
fn walk_folder_for_facts(
    fs: &dyn ScopeFs,
    ctx: &OpContext,
    root: &Path,
    dir: &Path,
    max_bytes: u64,
    walk: &mut FactsWalk,
) -> Result<(), CoreError> {
    if walk.truncated {
        return Ok(());
    }
    let Ok(mut entries) = fs.read_dir(dir) else {
        return Ok(());
    };
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in entries {
        ctx.checkpoint()?;
        if walk.truncated {
            return Ok(());
        }
        let path = dir.join(&entry.name);
        match entry.kind {
            FileKind::Symlink => {
                let is_file = fs
                    .canonicalize(&path)
                    .ok()
                    .and_then(|target| fs.symlink_metadata(&target).ok())
                    .is_some_and(|m| m.kind == FileKind::File);
                if is_file {
                    walk.file_count += 1;
                    if walk.file_count as usize >= MAX_FOLDER_FILES {
                        walk.truncated = true;
                    }
                }
            }
            FileKind::Dir => {
                walk_folder_for_facts(fs, ctx, root, &path, max_bytes, walk)?;
            }
            FileKind::File => {
                let Ok(meta) = fs.symlink_metadata(&path) else {
                    continue;
                };
                // Enforce the remaining byte budget before queuing the file,
                // not after: a single oversized file must never be added to
                // the hash queue, only counted as the reason the walk
                // stopped.
                let remaining = max_bytes.saturating_sub(walk.total_bytes);
                if meta.len > remaining {
                    walk.truncated = true;
                    continue;
                }
                walk.total_bytes += meta.len;
                walk.file_count += 1;
                if let Some(modified) = meta.modified {
                    if walk.newest.is_none_or(|n| modified > n) {
                        walk.newest = Some(modified);
                    }
                }
                if let Ok(rel) = path.strip_prefix(root) {
                    walk.hashable.push(HashableFile {
                        rel_path: rel.to_path_buf(),
                        abs_path: path.clone(),
                        len: meta.len,
                    });
                }
                if walk.file_count as usize >= MAX_FOLDER_FILES || walk.total_bytes >= max_bytes {
                    walk.truncated = true;
                }
            }
            FileKind::Other => {}
        }
    }
    Ok(())
}

/// sha256 over the sorted (relative path, bytes) pairs of a skill folder.
/// Every record is length-framed - `u64 LE len(rel_path) || rel_path bytes
/// || u64 LE file_len || file bytes` - so that, say, a file "a" containing
/// "bc" hashes differently from a file "ab" containing "c". Bytes are read
/// one file at a time rather than held in memory all at once. `max_bytes`
/// bounds the total file bytes read across every file; the truncation edge
/// case (a file so large only part of it fits the remaining byte budget)
/// degrades to "read nothing further from this file onward", since
/// [`ScopeFs::read_capped`] has no partial-read primitive. Checks
/// [`OpContext::checkpoint`] once per file, close enough granularity for a
/// folder of many small files; cancellation surfaces as an error rather
/// than a digest over a prefix of `files`, for the same reason
/// [`walk_folder_for_facts`] never returns a partial `walk` as a success.
fn content_hash_from_walk(
    mut files: Vec<HashableFile>,
    fs: &dyn ScopeFs,
    ctx: &OpContext,
    max_bytes: u64,
) -> Result<String, CoreError> {
    files.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    let mut hasher = Sha256::new();
    let mut remaining = max_bytes;
    for file in &files {
        ctx.checkpoint()?;
        let rel_bytes = file.rel_path.to_string_lossy().into_owned().into_bytes();
        hasher.update((rel_bytes.len() as u64).to_le_bytes());
        hasher.update(&rel_bytes);
        hasher.update(file.len.to_le_bytes());
        if remaining == 0 {
            continue;
        }
        match fs.read_capped(&file.abs_path, remaining.min(file.len)) {
            Ok(bytes) => {
                remaining = remaining.saturating_sub(bytes.len() as u64);
                hasher.update(&bytes);
            }
            Err(_) => remaining = 0,
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Every content fact about a skill folder that [`DeploymentDto`] carries,
/// gathered from one `SKILL.md` read and one folder walk.
#[derive(Default)]
struct ContentFacts {
    frontmatter: Option<frontmatter::SkillFrontmatter>,
    frontmatter_fields: BTreeMap<String, String>,
    has_spec: bool,
    folder_bytes: u64,
    file_count: u32,
    skill_md_tokens: u32,
    description_tokens: u32,
    content_hash: String,
    modified_at: Option<DateTime<Utc>>,
    folder_truncated: bool,
}

/// Walks `skill_dir` and derives every [`ContentFacts`] field from the
/// already-read `skill_md_bytes`/`parsed` (so the caller's own `SKILL.md`
/// read and parse, already needed for `description`/`spec_violations`, is
/// never repeated here). `skill_md_bytes`' own length is deducted from the
/// folder walk's byte budget first, since the walk re-reads/hashes
/// `SKILL.md` as part of the folder.
fn compute_content_facts(
    fs: &dyn ScopeFs,
    ctx: &OpContext,
    skill_dir: &Path,
    skill_md_bytes: &[u8],
    skill_md_truncated: bool,
    parsed: &frontmatter::FrontmatterParseResult,
) -> Result<ContentFacts, CoreError> {
    let content = String::from_utf8_lossy(skill_md_bytes).into_owned();
    let frontmatter = parsed.as_frontmatter().cloned();
    let name_for_tokens = frontmatter
        .as_ref()
        .and_then(|f| f.name.clone())
        .unwrap_or_default();
    let description_for_tokens = frontmatter
        .as_ref()
        .and_then(|f| f.description.clone())
        .unwrap_or_default();

    let mut walk = FactsWalk::default();
    walk_folder_for_facts(
        fs,
        ctx,
        skill_dir,
        skill_dir,
        MAX_FOLDER_BYTES.saturating_sub(skill_md_bytes.len() as u64),
        &mut walk,
    )?;

    let tok = tokenizer();
    Ok(ContentFacts {
        frontmatter_fields: frontmatter::frontmatter_fields(&content),
        has_spec: has_spec(fs, skill_dir),
        folder_bytes: walk.total_bytes,
        file_count: walk.file_count,
        skill_md_tokens: count_tokens(&content, tok),
        description_tokens: count_tokens(
            &format!("{name_for_tokens}: {description_for_tokens}"),
            tok,
        ),
        content_hash: content_hash_from_walk(walk.hashable, fs, ctx, MAX_FOLDER_BYTES)?,
        modified_at: walk.newest,
        folder_truncated: walk.truncated || skill_md_truncated,
        frontmatter,
    })
}

/// Whether `dir` sits inside a git working tree (a `.git` file or directory
/// on some ancestor up to the scope root), bounded to the scope: this walks
/// through [`ScopedReads`], so it never reads real directories above the
/// scope's home or projects. See [`ScopeFs::ancestor_holds`].
fn in_git_repo(fs: &dyn ScopeFs, scope: &NormalizedScope, dir: &Path) -> bool {
    ScopedReads::new(fs, scope)
        .ancestor_holds(dir, ".git")
        .unwrap_or(false)
}

/// Whether `path`'s components contain `.agents/skills` back to back -
/// i.e. the path resolves into the shared dotagents-managed root.
fn resolves_into_dotagents(path: &Path) -> bool {
    let comps: Vec<String> = path
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    comps
        .windows(2)
        .any(|w| w[0] == ".agents" && w[1] == "skills")
}

/// The install-source badge is a projection of the owner classification, not
/// a second classifier: the owner lookup already answered "which ledger, in
/// which scope, claims this folder", and that is the same question. A
/// separate lock-file check here would be scope-blind - one home lock file
/// tested against every root's bare name - and would badge a per-harness
/// manual folder as skills.sh whenever the home lock names a same-named
/// skill. Every `Ambiguous` origin (a dual claim, an unnamed dotagents
/// ledger beside the universal root, a link back into it) reads as dotagents
/// because each one is a dotagents-managed root with no single owner row.
fn source_kind_from_owner(owner: LifecycleOwnerKind) -> SourceKind {
    match owner {
        LifecycleOwnerKind::Plugin => SourceKind::Plugin,
        LifecycleOwnerKind::Fork => SourceKind::Fork,
        LifecycleOwnerKind::SkillsSh => SourceKind::SkillsSh,
        LifecycleOwnerKind::InRepo => SourceKind::InRepo,
        LifecycleOwnerKind::Dotagents
        | LifecycleOwnerKind::WildcardDotagents
        | LifecycleOwnerKind::Ambiguous => SourceKind::Dotagents,
        LifecycleOwnerKind::Copy | LifecycleOwnerKind::Manual => SourceKind::Manual,
    }
}

/// Recomputes the whole-folder `content_hash` for `skill_dir`, outside of a
/// scan. Mutation guards (`skill_add_operation`, `skill_independent_copy`)
/// need the live hash before writing over an existing deployment, with no
/// `Inventory` in hand.
///
/// Takes the filesystem port directly, rather than a [`Runtime`], because it
/// is not a scoped operation: `skill_dir` is NOT confined to a scope. That is
/// deliberate and load-bearing: `skill_independent_copy` hashes a staging
/// directory that need not sit under the scope root, and a scope-confined
/// read would report it as missing. The caller therefore owns the choice of
/// path - pass a directory the user's own action named, never one taken
/// from untrusted input.
pub fn skill_content_hash(
    fs: &dyn ScopeFs,
    ctx: &OpContext,
    skill_dir: &Path,
) -> Result<String, CoreError> {
    // The Add path walks a folder that may be large and must stay
    // interruptible, which is why this takes a context like every other op
    // rather than making the caller choose between a digest and a
    // cancellable one.
    ctx.checkpoint()?;
    let skill_md = skill_dir.join("SKILL.md");
    let (bytes, _truncated) = fs
        .read_prefix(&skill_md, SKILL_MD_MAX_BYTES)
        .map_err(|e| CoreError::io(skill_md.clone(), e))?;
    let mut walk = FactsWalk::default();
    walk_folder_for_facts(
        fs,
        ctx,
        skill_dir,
        skill_dir,
        MAX_FOLDER_BYTES.saturating_sub(bytes.len() as u64),
        &mut walk,
    )?;
    content_hash_from_walk(walk.hashable, fs, ctx, MAX_FOLDER_BYTES)
}

/// Runs `scan` and derives issues from the inventory.
///
/// Preconditions: same as [`scan`]. Issue derivation itself reads no root
/// directory and takes no lease of its own; it only re-reads a candidate
/// `SKILL.md` already named by the inventory, to test whether
/// [`propose_colon_scalar_repair`] would fix its frontmatter
/// ([`IssueKind::RepairableFrontmatter`]) and whether a broken-looking link's
/// target exists at all ([`IssueKind::BrokenLink`] vs
/// [`IssueKind::UnreadableLink`]). A read that fails there is treated as "not
/// repairable"/"not readable" rather than surfaced as an error: `diagnose`
/// never fails just because one deployment's extra read did.
pub fn diagnose(rt: &Runtime, ctx: &OpContext, req: &ScanRequest) -> Result<Diagnosis, CoreError> {
    let inventory = scan(rt, ctx, req)?;
    ctx.checkpoint()?;
    let issues = derive_issues(rt.ports.fs.as_ref(), &inventory);
    Ok(Diagnosis { inventory, issues })
}

/// Pure(ish) issue derivation over an [`Inventory`]; see [`diagnose`] for the
/// two rules that re-read a file. Issues are sorted by severity (`Error`,
/// `Warning`, `Off`), then skill name, then kind, matching the doc comment on
/// [`ops::diagnose`](diagnose).
fn derive_issues(fs: &dyn ScopeFs, inventory: &Inventory) -> Vec<Issue> {
    let mut issues = Vec::new();

    for skill in &inventory.skills {
        for deployment in &skill.deployments {
            if let Some(issue) = broken_or_unreadable_link_issue(fs, skill, deployment) {
                issues.push(issue);
            }
            for violation in &deployment.spec_violations {
                issues.push(Issue {
                    kind: IssueKind::SpecViolation,
                    severity: Severity::Warning,
                    skill: skill.name.clone(),
                    deployment_id: Some(deployment.id.clone()),
                    message: violation.clone(),
                    next_action: NextAction::None,
                });
            }
            if let Some(issue) = repairable_frontmatter_issue(fs, skill, deployment) {
                issues.push(issue);
            }
            if matches!(deployment.root.kind, RootKind::Parked) {
                issues.push(Issue {
                    kind: IssueKind::Parked,
                    severity: Severity::Off,
                    skill: skill.name.clone(),
                    deployment_id: Some(deployment.id.clone()),
                    message: format!("{} is parked", deployment.path.display()),
                    next_action: NextAction::None,
                });
            }
            if deployment.disabled_by.is_some() {
                issues.push(Issue {
                    kind: IssueKind::Disabled,
                    severity: Severity::Off,
                    skill: skill.name.clone(),
                    deployment_id: Some(deployment.id.clone()),
                    message: format!("{} is disabled", deployment.path.display()),
                    next_action: NextAction::None,
                });
            }
        }
        issues.extend(duplicate_issues(skill));
    }

    for observation in &inventory.observations {
        if observation.message.starts_with(TRUNCATED_SKILL_MD_PREFIX) {
            continue;
        }
        issues.push(Issue {
            kind: IssueKind::RootUnreadable,
            severity: Severity::Error,
            skill: SkillName(String::new()),
            deployment_id: None,
            message: observation.message.clone(),
            next_action: NextAction::Rescan,
        });
    }

    issues.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.skill.0.cmp(&b.skill.0))
            .then_with(|| format!("{:?}", a.kind).cmp(&format!("{:?}", b.kind)))
    });
    issues
}

/// A linked deployment with no content fingerprint means its target could
/// not be read; [`fs`] tells apart "the target is missing"
/// ([`IssueKind::BrokenLink`]) from "the target exists but couldn't be read"
/// ([`IssueKind::UnreadableLink`]), which the fingerprint-less deployment
/// alone cannot.
fn broken_or_unreadable_link_issue(
    fs: &dyn ScopeFs,
    skill: &InstalledSkillDto,
    deployment: &DeploymentDto,
) -> Option<Issue> {
    if !matches!(deployment.backing, BackingRelationship::LinkedTo)
        || deployment.content_fingerprint.is_some()
    {
        return None;
    }
    let link_target = deployment.link_target.as_deref()?;
    let (kind, message) = if fs.symlink_metadata(link_target).is_ok() {
        (
            IssueKind::UnreadableLink,
            format!("{} could not be read", link_target.display()),
        )
    } else {
        (
            IssueKind::BrokenLink,
            format!("{} links to a missing target", deployment.path.display()),
        )
    };
    Some(Issue {
        kind,
        severity: Severity::Error,
        skill: skill.name.clone(),
        deployment_id: Some(deployment.id.clone()),
        message,
        next_action: NextAction::RepairLink {
            deployment_id: deployment.id.clone(),
        },
    })
}

/// `true` when one of `deployment`'s spec violations is the "invalid YAML
/// frontmatter" message [`crate::frontmatter::validate_skill`] emits.
fn has_invalid_yaml_violation(deployment: &DeploymentDto) -> bool {
    deployment
        .spec_violations
        .iter()
        .any(|v| v.starts_with("invalid YAML frontmatter"))
}

/// Ports the desktop's `apply_modes`
/// (`apps/desktop/src-tauri/src/skills/skill_frontmatter_repair.rs`) gate
/// and per-owner mode selection byte-for-byte, so `diagnose`'s
/// `PreviewRepair` next action and `preview_frontmatter_repair`/
/// `apply_frontmatter_repair`'s own gate agree by construction: a plugin
/// deployment, a symlink, a whole-directory link, or a read-only deployment
/// not owned by `Manual` gets no modes at all; a `SkillsSh`/`Dotagents`
/// -owned canonical universal deployment gets `ForkAndFix`+
/// `FixInstalledCopy`; a `Copy`/`Fork`/`Manual`-owned deployment gets
/// `ApplyFix`; anything else gets none. The desktop has no core-side
/// equivalent field this omits - every condition in `apply_modes` maps to a
/// [`DeploymentDto`] field.
fn desktop_repair_apply_modes(deployment: &DeploymentDto) -> Vec<RepairApplyMode> {
    if deployment.plugin.is_some()
        || deployment.link_target.is_some()
        || deployment.shared_via_whole_dir_link
        || (deployment.mutability == DeploymentMutability::ReadOnly
            && deployment.owner_kind != LifecycleOwnerKind::Manual)
    {
        return vec![];
    }
    match deployment.owner_kind {
        LifecycleOwnerKind::SkillsSh | LifecycleOwnerKind::Dotagents => {
            if matches!(deployment.root.scope, RootScope::Global)
                && deployment.destination == SkillDestination::Universal
                && matches!(deployment.backing, BackingRelationship::Canonical)
            {
                vec![
                    RepairApplyMode::ForkAndFix,
                    RepairApplyMode::FixInstalledCopy,
                ]
            } else {
                vec![]
            }
        }
        LifecycleOwnerKind::Copy | LifecycleOwnerKind::Fork | LifecycleOwnerKind::Manual => {
            vec![RepairApplyMode::ApplyFix]
        }
        _ => vec![],
    }
}

/// Re-reads `deployment`'s `SKILL.md` and checks whether
/// [`propose_colon_scalar_repair`] can fix it. `None` when the deployment's
/// frontmatter parsed fine, the file can no longer be read, no safe unique
/// repair exists, or [`desktop_repair_apply_modes`] would refuse every mode
/// this core build can write (`ApplyFix`) - otherwise `preview_repair` would
/// contradict this very diagnosis by refusing the action `diagnose` just
/// proposed.
fn repairable_frontmatter_issue(
    fs: &dyn ScopeFs,
    skill: &InstalledSkillDto,
    deployment: &DeploymentDto,
) -> Option<Issue> {
    if !has_invalid_yaml_violation(deployment) {
        return None;
    }
    if !desktop_repair_apply_modes(deployment).contains(&RepairApplyMode::ApplyFix) {
        return None;
    }
    let bytes = fs
        .read_capped(&deployment.path.join("SKILL.md"), SKILL_MD_MAX_BYTES)
        .ok()?;
    let content = String::from_utf8_lossy(&bytes);
    let (_, reason) = propose_colon_scalar_repair(&content).ok()?;
    Some(Issue {
        kind: IssueKind::RepairableFrontmatter,
        severity: Severity::Warning,
        skill: skill.name.clone(),
        deployment_id: Some(deployment.id.clone()),
        message: reason,
        next_action: NextAction::PreviewRepair {
            deployment_id: deployment.id.clone(),
        },
    })
}

/// One [`IssueKind::Duplicate`] per `(harness, scope)` group that has more
/// than one `Canonical` or `Independent` deployment of this skill: two
/// copies a reader would see as the same name installed twice.
fn duplicate_issues(skill: &InstalledSkillDto) -> Vec<Issue> {
    let mut groups: BTreeMap<(Option<String>, String), Vec<&DeploymentDto>> = BTreeMap::new();
    for deployment in &skill.deployments {
        if !matches!(
            deployment.backing,
            BackingRelationship::Canonical | BackingRelationship::Independent
        ) {
            continue;
        }
        let harness = deployment.harness.as_ref().map(|h| h.as_str().to_string());
        let scope = match &deployment.root.scope {
            RootScope::Global => "global".to_string(),
            RootScope::Project(p) => format!("project:{}", p.0.display()),
        };
        groups.entry((harness, scope)).or_default().push(deployment);
    }
    groups
        .into_values()
        .filter(|group| group.len() > 1)
        .map(|group| Issue {
            kind: IssueKind::Duplicate,
            severity: Severity::Warning,
            skill: skill.name.clone(),
            deployment_id: None,
            message: format!(
                "{} is installed {} times in the same scope",
                skill.name.0,
                group.len()
            ),
            next_action: NextAction::None,
        })
        .collect()
}

/// Reports harness facts from the catalog, optionally probing the machine.
///
/// Preconditions: none. With `observe = false` and no `tools` this reads
/// nothing. A harness without a catalog row is
/// [`ErrorCode::InvalidRequest`]; a `tools` list without a `ToolLookup`
/// port is [`ErrorCode::Unsupported`].
pub fn capabilities(
    rt: &Runtime,
    ctx: &OpContext,
    req: &CapabilitiesRequest,
) -> Result<Capabilities, CoreError> {
    ctx.checkpoint()?;
    let catalog = &rt.ports.catalog;
    if let Some(unknown) = req.harnesses.iter().find(|id| catalog.get(id).is_none()) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!("no catalog row for harness `{}`", unknown.as_str()),
        ));
    }
    let tools = match (&rt.ports.tools, req.tools.is_empty()) {
        (_, true) => Vec::new(),
        (None, false) => {
            return Err(CoreError::new(
                ErrorCode::Unsupported,
                "tool lookup needs a ToolLookup port; this adapter has none",
            ))
        }
        (Some(lookup), false) => req
            .tools
            .iter()
            .map(|name| ToolAvailability {
                name: name.clone(),
                path: lookup.find_binary(name),
            })
            .collect(),
    };
    let harnesses = catalog
        .facts
        .iter()
        .filter(|f| req.harnesses.is_empty() || req.harnesses.contains(&f.id))
        .map(|f| {
            let observed = req.observe.then(|| observe_harness(rt, f));
            CapabilityReport::from_facts(f, observed)
        })
        .collect();
    Ok(Capabilities { harnesses, tools })
}

/// Probes the machine for one harness's [`HarnessObserved`] facts.
///
/// `config_present` is `true` when any of the harness's global roots exists
/// under the home; a project-scoped root says nothing about the harness
/// itself being configured, so only [`ScopeLevel::Global`] roots are probed.
/// `config_writable` stays [`Support::Unknown`]: telling a parseable config
/// apart from a `.jsonc` one with comments needs a per-harness parser this
/// operation does not have. `runner_binary` resolves through the
/// [`crate::ports::ToolLookup`] port when both a binary name and the port
/// are present.
fn observe_harness(rt: &Runtime, facts: &HarnessFacts) -> HarnessObserved {
    let config_present = facts.roots.iter().any(|root| {
        root.level == ScopeLevel::Global
            && rt
                .ports
                .fs
                .symlink_metadata(&rt.scope.home.canonical.join(&root.relative_path))
                .is_ok()
    });
    let runner_binary = facts.runner.binary.as_deref().and_then(|binary| {
        rt.ports
            .tools
            .as_deref()
            .and_then(|lookup| lookup.find_binary(binary))
    });
    HarnessObserved {
        config_present,
        config_writable: Support::Unknown,
        runner_binary,
    }
}

/// Finds the one deployment matching `id` in `inventory`, with its skill
/// name. Mirrors [`crate::ports::MutationSession::resolve_exact`] for the
/// read-only paths (`preview_frontmatter_repair`, `list_events`'s drift
/// check) that scan without taking the exclusive lease.
fn resolve_deployment<'a>(
    inventory: &'a Inventory,
    id: &DeploymentId,
) -> Result<(&'a SkillName, &'a DeploymentDto), CoreError> {
    let mut found = inventory
        .skills
        .iter()
        .flat_map(|s| s.deployments.iter().map(move |d| (&s.name, d)));
    let mut matches = found.by_ref().filter(|(_, d)| &d.id == id);
    match (matches.next(), matches.next()) {
        (Some(one), None) => Ok(one),
        (None, _) => Err(CoreError::new(
            ErrorCode::AmbiguousTarget,
            format!("no deployment matches {}", id.as_str()),
        )),
        (Some(_), Some(_)) => Err(CoreError::new(
            ErrorCode::AmbiguousTarget,
            format!("more than one deployment matches {}", id.as_str()),
        )),
    }
}

/// Reads `SKILL.md` under `dir`, fails with [`ErrorCode::Io`] when absent or
/// unreadable, and with [`ErrorCode::ExecutionFailed`] when it is not UTF-8.
fn read_skill_md_text(
    fs: &dyn ScopeFs,
    dir: &Path,
) -> Result<(PathBuf, Vec<u8>, String), CoreError> {
    let path = dir.join("SKILL.md");
    let bytes = fs
        .read_capped(&path, SKILL_MD_MAX_BYTES)
        .map_err(|e| CoreError::io(&path, e))?;
    let text = String::from_utf8(bytes.clone()).map_err(|_| {
        CoreError::new(ErrorCode::ExecutionFailed, "SKILL.md is not valid UTF-8").at(&path)
    })?;
    Ok((path, bytes, text))
}

/// Builds a proposal id: sha256 over deployment id, path, owner id, owner
/// kind, the expected fingerprint, and the proposed text, matching
/// [`crate::identity::ProposalId`]'s invariant.
fn proposal_id_for(
    deployment: &DeploymentDto,
    expected_fingerprint: &Fingerprint,
    proposed_content: &str,
) -> crate::identity::ProposalId {
    let raw = format!(
        "{}|{}|{:?}|{:?}|{}|{}",
        deployment.id.as_str(),
        deployment.path.display(),
        deployment.owner_id,
        deployment.owner_kind,
        expected_fingerprint.as_str(),
        proposed_content,
    );
    crate::identity::ProposalId(crate::identity::sha256_hex(raw.as_bytes()))
}

/// Proposes a frontmatter fix for one deployment without writing.
///
/// Preconditions: shared lease. The deployment must resolve exactly once.
/// The only fix known today is [`propose_colon_scalar_repair`]'s unquoted
/// `: ` repair, so a deployment whose `SKILL.md` does not match that one
/// safe, deterministic shape fails with [`ErrorCode::Unsupported`]. Per the
/// migration mapping, the core applies to a deployment only in this PR: a
/// read-only deployment is [`ErrorCode::Unsupported`] rather than proposing
/// a fork or a ledger-owned copy the core cannot yet write.
pub fn preview_frontmatter_repair(
    rt: &Runtime,
    ctx: &OpContext,
    req: &RepairPreviewRequest,
) -> Result<FrontmatterRepairPreview, CoreError> {
    ctx.checkpoint()?;
    let _guard = acquire_shared(rt.ports.leases.as_ref(), &rt.scope)?;
    let inventory = scan_inner(
        rt,
        ctx,
        &ScanRequest {
            skills: Vec::new(),
            timings: false,
        },
    )?;
    let (_skill, deployment) = resolve_deployment(&inventory, &req.deployment_id)?;
    // `desktop_repair_apply_modes` is the desktop's real gate (plugin,
    // symlink, whole-dir-link, and the `ReadOnly && owner != Manual`
    // carve-out), not just a `mutability` check - see its doc comment. Core
    // only has a writer for `ApplyFix` (`apply_frontmatter_repair`);
    // `ForkAndFix`/`FixInstalledCopy` need a fork writer / ledger writer
    // this core build doesn't have yet, so a deployment the desktop would
    // offer only those modes for is `Unsupported` here rather than
    // approximated as `ApplyFix`.
    if !desktop_repair_apply_modes(deployment).contains(&RepairApplyMode::ApplyFix) {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "this deployment is not repairable by this core build",
        )
        .at(&deployment.path));
    }
    let fs = rt.ports.fs.as_ref();
    let (path, bytes, content) = read_skill_md_text(fs, &deployment.path)?;
    let (proposed_content, reason) = propose_colon_scalar_repair(&content)
        .map_err(|message| CoreError::new(ErrorCode::Unsupported, message).at(&path))?;
    let expected_fingerprint = Fingerprint::of_bytes(&bytes);
    let proposed_fingerprint = Fingerprint::of_bytes(proposed_content.as_bytes());
    let proposal_id = proposal_id_for(deployment, &expected_fingerprint, &proposed_content);
    let diff = similar::TextDiff::from_lines(&content, &proposed_content)
        .unified_diff()
        .header("original", "proposed")
        .to_string();
    Ok(FrontmatterRepairPreview {
        proposal_id,
        deployment_id: deployment.id.clone(),
        path,
        scope: deployment.root.scope.clone(),
        reason,
        owner_id: deployment.owner_id.clone(),
        owner_kind: deployment.owner_kind,
        expected_fingerprint,
        proposed_fingerprint,
        original_content: content,
        proposed_content,
        diff,
        // Narrowing per the migration mapping: the core writes a
        // deployment only in this PR. `FixInstalledCopy`/`ForkAndFix` need
        // a ledger writer or a fork writer in core, which land in a later
        // phase; the adapter resolves those modes itself until then.
        allowed_apply_modes: vec![RepairApplyMode::ApplyFix],
        managed_update_warning: None,
    })
}

/// Applies a previewed fix under the exclusive lease.
///
/// Preconditions: exclusive lease; `preview.expected_fingerprint` still
/// matches the file ([`ErrorCode::StaleProposal`] otherwise); owner unchanged
/// ([`ErrorCode::OwnershipChanged`] otherwise); mode allowed. Records
/// `repair_skill_frontmatter` before the write and finishes it after.
pub fn apply_frontmatter_repair(
    rt: &Runtime,
    ctx: &OpContext,
    req: &RepairApplyRequest,
) -> Result<RepairOutcome, CoreError> {
    ctx.checkpoint()?;
    let preview = &req.preview;
    if !preview.allowed_apply_modes.contains(&req.mode) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!("{:?} is not in the preview's allowed apply modes", req.mode),
        ));
    }
    // Narrowing per the migration mapping: the core writes a deployment
    // only in this PR; `preview_frontmatter_repair` never offers the other
    // two modes, so this can only fire on a hand-built request.
    if req.mode != RepairApplyMode::ApplyFix {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            format!("{:?} is not implemented by this core build", req.mode),
        ));
    }

    let mut session = crate::ports::MutationSession::begin(rt, ctx)?;
    let (skill, deployment) = resolve_deployment(&session.fresh, &preview.deployment_id)?;
    if deployment.owner_id != preview.owner_id || deployment.owner_kind != preview.owner_kind {
        return Err(CoreError::new(
            ErrorCode::OwnershipChanged,
            "the deployment's owner changed since the preview",
        )
        .at(&deployment.path));
    }
    // Re-checked under the exclusive lease: the deployment could have
    // become a symlink, gone plugin-owned, or otherwise stopped satisfying
    // `desktop_repair_apply_modes` between preview and apply even with the
    // owner unchanged.
    if !desktop_repair_apply_modes(deployment).contains(&RepairApplyMode::ApplyFix) {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "this deployment is not repairable by this core build",
        )
        .at(&deployment.path));
    }

    let fs = rt.ports.fs.as_ref();
    let (path, bytes, content) = read_skill_md_text(fs, &deployment.path)?;
    let live_fingerprint = Fingerprint::of_bytes(&bytes);
    if live_fingerprint == preview.proposed_fingerprint {
        return Ok(RepairOutcome::AlreadyApplied {
            deployment_id: deployment.id.clone(),
        });
    }
    if live_fingerprint != preview.expected_fingerprint {
        return Err(CoreError::new(
            ErrorCode::StaleProposal,
            "SKILL.md changed since the preview was generated",
        )
        .at(&path));
    }
    // The repair is deterministic: recompute it from the bytes on disk and
    // compare the resulting proposal id, rather than trusting the caller's
    // `preview.proposed_content`, which is never written as sent.
    let (proposed_content, _reason) = propose_colon_scalar_repair(&content)
        .map_err(|message| CoreError::new(ErrorCode::StaleProposal, message).at(&path))?;
    let proposal_id = proposal_id_for(deployment, &live_fingerprint, &proposed_content);
    if proposal_id != preview.proposal_id {
        return Err(CoreError::new(
            ErrorCode::StaleProposal,
            "the recomputed repair no longer matches the preview",
        )
        .at(&path));
    }

    let deployment_id = deployment.id.clone();
    let skill = skill.clone();
    let harness = deployment.harness.clone();
    let scope_label = scope_label(&deployment.root.scope).to_string();
    let project_path = match &deployment.root.scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.clone()),
    };

    let id = rt.ports.ids.next_event_id();
    let manifest = session
        .store
        .backup_paths(&session.guard, &id, std::slice::from_ref(&path))?;
    let pre_fingerprint = manifest.entries.first().and_then(|e| e.fingerprint.clone());
    let inverse = crate::events::restore_backup_inverse(&path, pre_fingerprint.as_ref(), None);
    let draft = crate::events::EventDraft {
        kind: crate::events::EventKind::RepairSkillFrontmatter,
        skill,
        harness,
        scope: Some(scope_label),
        project_path,
        payload: serde_json::json!({
            "deployment_id": deployment_id.as_str(),
            "mode": req.mode,
        }),
        inverse: Some(inverse),
        backup_dir: Some(manifest.backup_dir.clone()),
    };
    session.store.record(&session.guard, &id, &draft)?;

    let scoped = crate::ports::confine(&rt.scope, fs, &path)?;
    fs.write_atomic(&session.guard, &scoped, proposed_content.as_bytes())
        .map_err(|e| CoreError::io(&path, e))?;

    // The recorded post-fingerprint must use the same tag+length-framed
    // scheme as `backup_paths`'s manifest entries (`hash_entry` on the host
    // side, `fingerprint_path` here): that's what `list_events`'s drift
    // check and `restore_event` compare a live path against, and it is not
    // the same hash as `Fingerprint::of_bytes` over the raw text used above
    // to compare `SKILL.md` bytes against `preview.proposed_fingerprint`.
    let post_fingerprint =
        crate::events::fingerprint_path(fs, &path)?.expect("just wrote this path; it exists");
    session.store.finish(
        &session.guard,
        &id,
        crate::events::EventStatus::Done,
        Some(post_fingerprint),
    )?;

    session.finish(rt, ctx);
    Ok(RepairOutcome::Applied {
        event_id: id,
        deployment_id,
    })
}

/// Lists history rows, newest first.
///
/// Preconditions: none. Returns an empty list when no store exists; never
/// creates one. `req.after` pages backwards through the log. With
/// `req.check_drift` each restorable row compares live fingerprints with
/// the recorded ones and reports [`crate::dto::DriftState`]; otherwise
/// `drift` is `Unchecked`.
pub fn list_events(
    rt: &Runtime,
    ctx: &OpContext,
    req: &ListEventsRequest,
) -> Result<Vec<EventDto>, CoreError> {
    ctx.checkpoint()?;
    let Some(store) = rt
        .ports
        .history
        .open(&rt.scope, HistoryAccess::ReadIfExists)?
    else {
        return Ok(Vec::new());
    };
    let filter = EventFilter {
        skill: req.skill.clone(),
        limit: if req.limit == 0 {
            DEFAULT_EVENT_LIMIT
        } else {
            req.limit
        },
        after: req.after.clone(),
    };
    let rows = store.list(&filter)?;
    let mut dtos: Vec<EventDto> = rows.iter().map(|r| r.to_dto()).collect();
    if req.check_drift {
        let fs = rt.ports.fs.as_ref();
        for (row, dto) in rows.iter().zip(dtos.iter_mut()) {
            let Some(inverse) = &row.inverse else {
                continue;
            };
            // Only the `restore_backup` shape (this PR's only writer) can be
            // drift-checked without kind-specific knowledge of what else an
            // event may have touched.
            let Some(obj) = inverse.as_object() else {
                continue;
            };
            if obj.get("op").and_then(|v| v.as_str()) != Some("restore_backup") {
                continue;
            }
            let (Some(path), Some(post)) = (
                obj.get("path").and_then(|v| v.as_str()),
                obj.get("post_fingerprint").and_then(|v| v.as_str()),
            ) else {
                continue;
            };
            let live = crate::events::fingerprint_path(fs, Path::new(path))?;
            let live = live.as_ref().map(|f| f.bare_hex()).unwrap_or("absent");
            dto.drift = if live == post {
                DriftState::Clean
            } else {
                DriftState::Drifted
            };
        }
    }
    Ok(dtos)
}

/// What a restore will do to the live file, decided before the claim on
/// `reverted_by` so the claim is only taken once nothing else can fail.
enum RestorePlan {
    /// The original event's backup recorded the path as absent.
    RemoveIfPresent,
    /// The bytes to write back, read from the original event's backup.
    Write(Vec<u8>),
}

/// Reverts one event.
///
/// Preconditions: exclusive lease; the claim on `reverted_by` succeeds
/// ([`ErrorCode::AlreadyReverted`] otherwise); the live fingerprint matches
/// the recorded one unless `force` ([`ErrorCode::DriftConflict`] otherwise).
/// The restore is itself an event with its own backup.
pub fn restore_event(
    rt: &Runtime,
    ctx: &OpContext,
    req: &RestoreRequest,
) -> Result<RestoreOutcome, CoreError> {
    ctx.checkpoint()?;
    let mut session = crate::ports::MutationSession::begin(rt, ctx)?;

    let target = session
        .store
        .get(&req.event_id)?
        .ok_or_else(|| CoreError::new(ErrorCode::InvalidRequest, "event not found"))?;
    match target.restore_capability() {
        crate::dto::RestoreCapability::Reverted { .. } => {
            return Err(CoreError::new(
                ErrorCode::AlreadyReverted,
                "this event was already reverted",
            ))
        }
        crate::dto::RestoreCapability::NoInverse | crate::dto::RestoreCapability::UnknownKind => {
            return Err(CoreError::new(
                ErrorCode::Unsupported,
                "this event cannot be restored",
            ))
        }
        crate::dto::RestoreCapability::Yes => {}
    }
    let inverse = target
        .inverse
        .as_ref()
        .expect("restore_capability() == Yes implies an inverse");
    let (path, pre, post) =
        crate::events::parse_restore_backup_inverse(inverse).ok_or_else(|| {
            CoreError::new(
                ErrorCode::Unsupported,
                "restore of this event kind is not implemented",
            )
        })?;

    let fs = rt.ports.fs.as_ref();
    let expected = post.as_deref().unwrap_or("absent");
    let live_fingerprint = crate::events::fingerprint_path(fs, &path)?;
    let live = live_fingerprint
        .as_ref()
        .map(|f| f.bare_hex())
        .unwrap_or("absent");
    if live != expected && !req.force {
        return Err(CoreError::new(
            ErrorCode::DriftConflict,
            "the file changed since this event; pass force to restore anyway",
        )
        .at(&path));
    }

    let restore_id = rt.ports.ids.next_event_id();
    // Backs up the file's current bytes under the restore event's own id
    // before touching it: with `force` this is exactly "the drifted bytes
    // are backed up first"; without drift it still gives the restore its
    // own undo.
    let manifest =
        session
            .store
            .backup_paths(&session.guard, &restore_id, std::slice::from_ref(&path))?;
    let restore_pre_fingerprint = manifest.entries.first().and_then(|e| e.fingerprint.clone());
    let restore_inverse =
        crate::events::restore_backup_inverse(&path, restore_pre_fingerprint.as_ref(), None);
    let draft = crate::events::EventDraft {
        kind: crate::events::EventKind::Restore,
        skill: target.skill.clone(),
        harness: target.harness.clone(),
        scope: target.scope.clone(),
        project_path: target.project_path.clone(),
        payload: serde_json::json!({ "target_event": target.id.0 }),
        inverse: Some(restore_inverse),
        backup_dir: Some(manifest.backup_dir.clone()),
    };
    session.store.record(&session.guard, &restore_id, &draft)?;

    // Every fallible, non-mutating step runs before the claim below: a
    // failure here must leave the target event revertible, not stuck behind
    // a claim nothing ever undoes.
    let scoped = crate::ports::confine(&rt.scope, fs, &path)?;
    let plan = match &pre {
        None => {
            // The original event's backup recorded the path as absent:
            // restoring means removing whatever is there now, if anything.
            RestorePlan::RemoveIfPresent
        }
        Some(_pre_fingerprint) => {
            let backup_dir = target.backup_dir.as_deref().ok_or_else(|| {
                CoreError::new(
                    ErrorCode::Io,
                    "a restore_backup inverse implies a backup_dir",
                )
                .at(&path)
            })?;
            let original_manifest = session.store.read_manifest(backup_dir)?;
            let entry = original_manifest
                .entries
                .iter()
                .find(|e| e.original == path)
                .ok_or_else(|| {
                    CoreError::new(
                        ErrorCode::Io,
                        "the original event's manifest has no entry for this path",
                    )
                    .at(&path)
                })?;
            let bytes = session
                .store
                .read_backup_bytes(backup_dir, &entry.relative)?;
            RestorePlan::Write(bytes)
        }
    };

    let claimed = session
        .store
        .claim_revert(&session.guard, &target.id, &restore_id)?;
    if !claimed {
        session.store.finish(
            &session.guard,
            &restore_id,
            crate::events::EventStatus::Failed,
            None,
        )?;
        return Err(CoreError::new(
            ErrorCode::AlreadyReverted,
            "this event was already reverted",
        ));
    }

    let mutation_result = match &plan {
        RestorePlan::RemoveIfPresent => {
            if fs.symlink_metadata(&path).is_ok() {
                fs.remove_file(&session.guard, &scoped)
                    .map_err(|e| CoreError::io(&path, e))
            } else {
                Ok(())
            }
        }
        RestorePlan::Write(bytes) => fs
            .write_atomic(&session.guard, &scoped, bytes)
            .map_err(|e| CoreError::io(&path, e)),
    };
    if let Err(err) = mutation_result {
        // The claim was already made durable, but nothing actually moved:
        // release it so the target event stays revertible on retry, and
        // report the original I/O error, not any failure of the release.
        let _ = session
            .store
            .release_revert(&session.guard, &target.id, &restore_id);
        let _ = session.store.finish(
            &session.guard,
            &restore_id,
            crate::events::EventStatus::Failed,
            None,
        );
        return Err(err);
    }

    let restored_fingerprint = crate::events::fingerprint_path(fs, &path)?;
    session.store.finish(
        &session.guard,
        &restore_id,
        crate::events::EventStatus::Done,
        restored_fingerprint,
    )?;

    session.finish(rt, ctx);
    Ok(RestoreOutcome {
        restore_event_id: restore_id,
        reverted_event_id: target.id,
        restored_paths: vec![path],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::RuntimeScope;
    use crate::testing::FixtureBuilder;

    fn scope() -> NormalizedScope {
        let fs = FixtureBuilder::new().dir("/h").build_fs();
        NormalizedScope::normalize(&RuntimeScope::fixture("/h"), &fs).unwrap()
    }

    #[test]
    fn partial_inventory_exits_4_without_an_error_value() {
        let inv = Inventory {
            skills: vec![],
            projects: vec![],
            completeness: Completeness::Partial,
            observations: vec![],
            timings: vec![],
        };
        let env = ResultEnvelope::from_result(
            Operation::Scan,
            &scope(),
            CorrelationId("c1".into()),
            Ok(inv),
        );
        assert_eq!(env.status, OpStatus::Partial);
        assert_eq!(env.exit_status(), 4);
    }

    #[test]
    fn diagnosis_with_a_warning_exits_1_and_partial_wins() {
        use crate::dto::{Issue, IssueKind, NextAction, Severity};
        use crate::identity::SkillName;
        let issue = Issue {
            kind: IssueKind::BrokenLink,
            severity: Severity::Warning,
            skill: SkillName("x".into()),
            deployment_id: None,
            message: "target missing".into(),
            next_action: NextAction::None,
        };
        let complete = Diagnosis {
            inventory: Inventory {
                skills: vec![],
                projects: vec![],
                completeness: Completeness::Complete,
                observations: vec![],
                timings: vec![],
            },
            issues: vec![issue.clone()],
        };
        let mut partial = complete.clone();
        partial.inventory.completeness = Completeness::Partial;
        let id = || CorrelationId("c3".into());
        let ok = ResultEnvelope::from_result(Operation::Diagnose, &scope(), id(), Ok(complete));
        assert_eq!(ok.exit_status(), 1);
        let part = ResultEnvelope::from_result(Operation::Diagnose, &scope(), id(), Ok(partial));
        assert_eq!(part.exit_status(), 4);
    }

    #[test]
    fn capabilities_rejects_unknown_harnesses_and_resolves_tools() {
        use crate::harness::HarnessCatalog;
        use crate::identity::AgentId;
        use crate::ports::Ports;
        use crate::testing::{
            FakeClock, FakeIds, FakeLease, FakeToolLookup, NoHistory, RecordingSink,
        };
        use std::path::Path;
        use std::sync::Arc;

        let fs = FixtureBuilder::new().dir("/h").build_fs();
        let mut ports = Ports {
            fs: Arc::new(fs),
            clock: Arc::new(FakeClock::at(0)),
            ids: Arc::new(FakeIds::default()),
            leases: Arc::new(FakeLease::default()),
            history: Arc::new(NoHistory),
            sink: Arc::new(RecordingSink::default()),
            spawner: None,
            discovery: None,
            tools: None,
            catalog: Arc::new(HarnessCatalog::builtin()),
        };
        let ctx = OpContext::uncancellable(CorrelationId("c4".into()));

        let rt = Runtime::new(&RuntimeScope::fixture("/h"), ports.clone()).unwrap();
        let unknown = CapabilitiesRequest {
            harnesses: vec![AgentId::from("windsurf")],
            ..Default::default()
        };
        let err = capabilities(&rt, &ctx, &unknown).unwrap_err();
        assert_eq!(err.code, ErrorCode::InvalidRequest);

        let with_tools = CapabilitiesRequest {
            tools: vec!["npx".into(), "dotagents".into()],
            ..Default::default()
        };
        let err = capabilities(&rt, &ctx, &with_tools).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unsupported, "no ToolLookup port");

        let mut lookup = FakeToolLookup::default();
        lookup
            .binaries
            .insert("npx".into(), "/usr/local/bin/npx".into());
        ports.tools = Some(Arc::new(lookup));
        let rt = Runtime::new(&RuntimeScope::fixture("/h"), ports).unwrap();
        let caps = capabilities(&rt, &ctx, &with_tools).unwrap();
        assert_eq!(caps.harnesses.len(), 6);
        assert_eq!(
            caps.tools[0].path.as_deref(),
            Some(Path::new("/usr/local/bin/npx"))
        );
        assert_eq!(caps.tools[1].path, None);
    }

    #[test]
    fn capabilities_observe_reports_config_presence_and_runner_binary() {
        use crate::harness::HarnessCatalog;
        use crate::identity::AgentId;
        use crate::ports::Ports;
        use crate::testing::{
            FakeClock, FakeIds, FakeLease, FakeToolLookup, NoHistory, RecordingSink,
        };
        use std::sync::Arc;

        // Claude Code's global root exists on disk; Codex's does not.
        let fs = FixtureBuilder::new().dir("/h/.claude/skills").build_fs();
        let mut lookup = FakeToolLookup::default();
        lookup
            .binaries
            .insert("claude".into(), "/usr/local/bin/claude".into());
        let ports = Ports {
            fs: Arc::new(fs),
            clock: Arc::new(FakeClock::at(0)),
            ids: Arc::new(FakeIds::default()),
            leases: Arc::new(FakeLease::default()),
            history: Arc::new(NoHistory),
            sink: Arc::new(RecordingSink::default()),
            spawner: None,
            discovery: None,
            tools: Some(Arc::new(lookup)),
            catalog: Arc::new(HarnessCatalog::builtin()),
        };
        let rt = Runtime::new(&RuntimeScope::fixture("/h"), ports).unwrap();
        let ctx = OpContext::uncancellable(CorrelationId("c5".into()));
        let req = CapabilitiesRequest {
            harnesses: vec![AgentId::from(AgentId::CLAUDE_CODE), AgentId::from("codex")],
            observe: true,
            ..Default::default()
        };
        let caps = capabilities(&rt, &ctx, &req).unwrap();

        let claude = caps
            .harnesses
            .iter()
            .find(|r| r.harness.as_str() == AgentId::CLAUDE_CODE)
            .unwrap();
        let observed = claude.observed.as_ref().expect("observe=true fills this");
        assert!(observed.config_present);
        assert_eq!(
            observed.runner_binary.as_deref(),
            Some(Path::new("/usr/local/bin/claude"))
        );

        let codex = caps
            .harnesses
            .iter()
            .find(|r| r.harness.as_str() == "codex")
            .unwrap();
        assert!(!codex.observed.as_ref().unwrap().config_present);
    }

    #[test]
    fn busy_lease_exits_3() {
        let env: ResultEnvelope<Inventory> = ResultEnvelope::from_result(
            Operation::Scan,
            &scope(),
            CorrelationId("c2".into()),
            Err(CoreError::new(ErrorCode::ScopeBusy, "held by pid 42")),
        );
        assert_eq!(env.exit_status(), 3);
        assert!(env.data.is_none());
    }

    mod scan_tests {
        use super::*;
        use crate::harness::HarnessCatalog;
        use crate::ports::{Clock, Ports};
        use crate::scope::ProjectSelection;
        use crate::testing::{FakeClock, FakeIds, FakeLease, NoHistory, RecordingSink};
        use chrono::Utc;
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        /// A clock whose `monotonic()` jumps forward by 3s on every call,
        /// so a scan that reads it more than once always blows the default
        /// 2s read budget - used to exercise the partial-completeness path
        /// without a real clock or a real timeout.
        struct SteppingClock(AtomicU64);

        impl SteppingClock {
            fn new() -> Self {
                SteppingClock(AtomicU64::new(0))
            }
        }

        impl Clock for SteppingClock {
            fn now(&self) -> chrono::DateTime<Utc> {
                Utc::now()
            }

            fn monotonic(&self) -> Duration {
                let calls = self.0.fetch_add(1, Ordering::SeqCst);
                Duration::from_millis(calls * 3_000)
            }
        }

        fn runtime_with(fs: crate::testing::FixtureFs, clock: Arc<dyn Clock>) -> Runtime {
            let ports = Ports {
                fs: Arc::new(fs),
                clock,
                ids: Arc::new(FakeIds::default()),
                leases: Arc::new(FakeLease::default()),
                history: Arc::new(NoHistory),
                sink: Arc::new(RecordingSink::default()),
                spawner: None,
                discovery: None,
                tools: None,
                catalog: Arc::new(HarnessCatalog::builtin()),
            };
            Runtime::new(&RuntimeScope::fixture("/h"), ports).unwrap()
        }

        pub(super) fn ctx() -> OpContext {
            OpContext::uncancellable(CorrelationId("scan-test".into()))
        }

        #[test]
        fn empty_scope_yields_no_skills_and_complete() {
            let fs = FixtureBuilder::new().dir("/h").build_fs();
            let rt = runtime_with(fs, Arc::new(FakeClock::at(0)));
            let inv = scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
            assert!(inv.skills.is_empty());
            assert_eq!(inv.completeness, Completeness::Complete);
        }

        #[test]
        fn finds_a_skill_under_a_harness_own_root() {
            let fs = FixtureBuilder::new()
                .dir("/h/.claude/skills/write-tests")
                .file(
                    "/h/.claude/skills/write-tests/SKILL.md",
                    b"---\nname: write-tests\ndescription: Writes tests.\n---\nBody.",
                )
                .build_fs();
            let rt = runtime_with(fs, Arc::new(FakeClock::at(0)));
            let inv = scan(&rt, &ctx(), &ScanRequest::default()).unwrap();

            assert_eq!(inv.skills.len(), 1);
            let skill = &inv.skills[0];
            assert_eq!(skill.name.0, "write-tests");
            assert_eq!(skill.description.as_deref(), Some("Writes tests."));
            assert_eq!(skill.deployments.len(), 1);
            let deployment = &skill.deployments[0];
            assert_eq!(
                deployment.harness.as_ref().map(AgentId::as_str),
                Some("claude-code")
            );
            assert!(deployment.spec_violations.is_empty());
            assert_eq!(inv.completeness, Completeness::Complete);
        }

        fn claude_plugin_fixture(settings_json: Option<&[u8]>) -> crate::testing::FixtureBuilder {
            let mut builder = FixtureBuilder::new()
                .dir("/h/.claude/plugins/cache/mp/plug/v1/skills/plugin-skill")
                .file(
                    "/h/.claude/plugins/cache/mp/plug/v1/.claude-plugin/plugin.json",
                    b"{\"name\": \"plug\"}",
                )
                .file(
                    "/h/.claude/plugins/cache/mp/plug/v1/skills/plugin-skill/SKILL.md",
                    b"---\nname: plugin-skill\ndescription: Plugin.\n---\n",
                );
            if let Some(bytes) = settings_json {
                builder = builder.file("/h/.claude/settings.json", bytes);
            }
            builder
        }

        fn plugin_enabled_in(inv: &Inventory) -> Option<bool> {
            inv.skills[0].deployments[0]
                .plugin
                .as_ref()
                .expect("plugin deployment")
                .enabled
        }

        #[test]
        fn claude_plugin_enabled_true_is_reported() {
            let fs = claude_plugin_fixture(Some(b"{\"enabledPlugins\": {\"plug@mp\": true}}"))
                .build_fs();
            let rt = runtime_with(fs, Arc::new(FakeClock::at(0)));
            let inv = scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
            assert_eq!(plugin_enabled_in(&inv), Some(true));
        }

        #[test]
        fn claude_plugin_enabled_false_is_reported() {
            let fs = claude_plugin_fixture(Some(b"{\"enabledPlugins\": {\"plug@mp\": false}}"))
                .build_fs();
            let rt = runtime_with(fs, Arc::new(FakeClock::at(0)));
            let inv = scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
            assert_eq!(plugin_enabled_in(&inv), Some(false));
        }

        #[test]
        fn claude_plugin_enabled_is_none_when_key_absent() {
            let fs = claude_plugin_fixture(Some(b"{\"enabledPlugins\": {}}")).build_fs();
            let rt = runtime_with(fs, Arc::new(FakeClock::at(0)));
            let inv = scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
            assert_eq!(plugin_enabled_in(&inv), None);
        }

        #[test]
        fn claude_plugin_enabled_is_none_when_settings_file_absent() {
            let fs = claude_plugin_fixture(None).build_fs();
            let rt = runtime_with(fs, Arc::new(FakeClock::at(0)));
            let inv = scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
            assert_eq!(plugin_enabled_in(&inv), None);
        }

        #[test]
        fn moved_aside_entries_are_visible_and_disabled() {
            // `MOVE_ASIDE_DIR_NAME` is itself dot-prefixed, so it is never
            // mistaken for a skill directory, but its children are walked
            // separately and reported as `StudioMoved`-disabled deployments
            // rather than being invisible.
            let fs = FixtureBuilder::new()
                .dir("/h/.claude/skills/.skill-studio-disabled/parked-skill")
                .file(
                    "/h/.claude/skills/.skill-studio-disabled/parked-skill/SKILL.md",
                    b"---\nname: parked-skill\ndescription: Parked.\n---\n",
                )
                .build_fs();
            let rt = runtime_with(fs, Arc::new(FakeClock::at(0)));
            let inv = scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
            assert_eq!(inv.skills.len(), 1);
            let deployment = &inv.skills[0].deployments[0];
            assert_eq!(deployment.disabled_by, Some(DisabledBy::StudioMoved));
        }

        #[test]
        fn skills_filter_restricts_results() {
            let fs = FixtureBuilder::new()
                .dir("/h/.claude/skills/write-tests")
                .file(
                    "/h/.claude/skills/write-tests/SKILL.md",
                    b"---\nname: write-tests\ndescription: Writes tests.\n---\n",
                )
                .dir("/h/.claude/skills/other-skill")
                .file(
                    "/h/.claude/skills/other-skill/SKILL.md",
                    b"---\nname: other-skill\ndescription: Other.\n---\n",
                )
                .build_fs();
            let rt = runtime_with(fs, Arc::new(FakeClock::at(0)));
            let req = ScanRequest {
                skills: vec![SkillName("write-tests".into())],
                timings: false,
            };
            let inv = scan(&rt, &ctx(), &req).unwrap();
            assert_eq!(inv.skills.len(), 1);
            assert_eq!(inv.skills[0].name.0, "write-tests");
        }

        #[test]
        fn exceeded_read_budget_marks_inventory_partial() {
            let fs = FixtureBuilder::new()
                .dir("/h/.claude/skills/write-tests")
                .file(
                    "/h/.claude/skills/write-tests/SKILL.md",
                    b"---\nname: write-tests\ndescription: Writes tests.\n---\n",
                )
                .build_fs();
            let rt = runtime_with(fs, Arc::new(SteppingClock::new()));
            let inv = scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
            assert_eq!(inv.completeness, Completeness::Partial);
            assert!(!inv.observations.is_empty());
        }

        /// A clock that reports zero elapsed time for the first
        /// `within_budget_calls` reads after `start`, then jumps past the
        /// default 2s budget on every call after that - lets a test trip the
        /// budget check at an exact target-group boundary instead of a
        /// guessed call count.
        struct BudgetAfterNClock {
            calls: AtomicU64,
            within_budget_calls: u64,
        }

        impl BudgetAfterNClock {
            fn new(within_budget_calls: u64) -> Self {
                BudgetAfterNClock {
                    calls: AtomicU64::new(0),
                    within_budget_calls,
                }
            }
        }

        impl Clock for BudgetAfterNClock {
            fn now(&self) -> chrono::DateTime<Utc> {
                Utc::now()
            }

            fn monotonic(&self) -> Duration {
                // Call 0 is `scan_inner`'s `start` read; calls
                // 1..=within_budget_calls are the global groups' budget
                // checks.
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if call <= self.within_budget_calls {
                    Duration::from_millis(0)
                } else {
                    Duration::from_millis(3_000)
                }
            }
        }

        #[test]
        fn global_roots_and_plugin_caches_scan_before_project_roots() {
            let fs = FixtureBuilder::new()
                .dir("/h/.claude/skills/home-skill")
                .file(
                    "/h/.claude/skills/home-skill/SKILL.md",
                    b"---\nname: home-skill\ndescription: Home.\n---\n",
                )
                .dir("/h/.claude/plugins/cache/mp/plug/v1/skills/plugin-skill")
                .file(
                    "/h/.claude/plugins/cache/mp/plug/v1/.claude-plugin/plugin.json",
                    br#"{"name":"plug"}"#,
                )
                .file(
                    "/h/.claude/plugins/cache/mp/plug/v1/skills/plugin-skill/SKILL.md",
                    b"---\nname: plugin-skill\ndescription: Plugin.\n---\n",
                )
                .dir("/h/proj/.claude/skills/project-skill")
                .file(
                    "/h/proj/.claude/skills/project-skill/SKILL.md",
                    b"---\nname: project-skill\ndescription: Project.\n---\n",
                )
                .build_fs();

            let mut scope = RuntimeScope::fixture("/h");
            scope.projects = ProjectSelection::Explicit {
                paths: vec![PathBuf::from("/h/proj")],
            };

            let ports_for = |clock: Arc<dyn Clock>| Ports {
                fs: Arc::new(fs.clone()),
                clock,
                ids: Arc::new(FakeIds::default()),
                leases: Arc::new(FakeLease::default()),
                history: Arc::new(NoHistory),
                sink: Arc::new(RecordingSink::default()),
                spawner: None,
                discovery: None,
                tools: None,
                catalog: Arc::new(HarnessCatalog::builtin()),
            };

            // `scan_targets`/`plugin_scan_targets` depend only on the
            // catalog and `rt.scope`, not on the clock, so a throwaway
            // clock is enough to count how many budget-check calls the
            // global groups make: the exact call count the calibrated
            // clock below needs to trip the budget right at the
            // project-scope boundary, rather than a guessed constant tied
            // to the builtin catalog's current harness count.
            let probe_rt = Runtime::new(&scope, ports_for(Arc::new(FakeClock::at(0)))).unwrap();
            let global_target_calls = scan_targets(&probe_rt)
                .iter()
                .filter(|t| matches!(t.scope, RootScope::Global))
                .count();
            let global_plugin_calls = plugin_scan_targets(&probe_rt)
                .iter()
                .filter(|t| matches!(t.scope, RootScope::Global))
                .count();
            let within_budget_calls = (global_target_calls + global_plugin_calls) as u64;

            let rt = Runtime::new(
                &scope,
                ports_for(Arc::new(BudgetAfterNClock::new(within_budget_calls))),
            )
            .unwrap();
            let inv = scan(&rt, &ctx(), &ScanRequest::default()).unwrap();

            assert_eq!(inv.completeness, Completeness::Partial);
            let names: Vec<&str> = inv.skills.iter().map(|s| s.name.0.as_str()).collect();
            assert!(names.contains(&"home-skill"));
            assert!(names.contains(&"plugin-skill"));
            assert!(!names.contains(&"project-skill"));
            assert!(!inv.observations.is_empty());
            for observation in &inv.observations {
                let root = observation
                    .root
                    .as_ref()
                    .expect("budget observations always name a root");
                assert!(matches!(root.scope, RootScope::Project(_)));
            }
        }

        #[test]
        fn timings_are_recorded_only_when_requested() {
            let fs = FixtureBuilder::new().dir("/h").build_fs();
            let rt = runtime_with(fs, Arc::new(FakeClock::at(0)));
            let req = ScanRequest {
                skills: Vec::new(),
                timings: true,
            };
            let inv = scan(&rt, &ctx(), &req).unwrap();
            assert_eq!(inv.timings.len(), 1);
            assert_eq!(inv.timings[0].phase, "scan");
        }
    }
}
