//! Operations and the result envelope.
//!
//! Each operation is a plain function over a [`Runtime`] and an
//! [`OpContext`]. Adapters wrap the result in a [`ResultEnvelope`] with
//! [`ResultEnvelope::from_result`]; the exit status is derived, never chosen.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
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
    FrontmatterRepairPreview, HarnessesRequest, InstalledSkillDto, Inventory, Issue, IssueKind,
    ListEventsRequest, NextAction, Observation, ParkOutcome, ParkRequest, PluginSourceDto,
    RepairApplyMode, RepairApplyRequest, RepairOutcome, RepairPreviewRequest, RestoreOutcome,
    RestoreRequest, ScanRequest, SetHarnessEnabledOutcome, SetHarnessEnabledRequest, Severity,
    Timing, UnparkOutcome, UnparkRequest,
};
use crate::error::{CoreError, ErrorCode, ErrorEntry};
use crate::events::EventFilter;
use crate::frontmatter;
use crate::frontmatter_repair::propose_colon_scalar_repair;
use crate::fsops;
use crate::harness::{
    builtin_adapters, Capabilities, CapabilityReport, DetectionPorts, DisabledBy, HarnessFacts,
    HarnessObserved, HarnessReport, RootRole, ScopeLevel, Support, ToolAvailability,
};
use crate::identity::{
    AgentId, BackingRelationship, CorrelationId, DeploymentId, DeploymentMutability, EventId,
    Fingerprint, LifecycleOwnerKind, OwnerId, PlanId, ProjectRef, RootKind, RootRef, RootScope,
    SkillDestination, SkillName, SourceKind, MOVE_ASIDE_DIR_NAME, PARKED_ROOT_RELATIVE,
    UNIVERSAL_ROOT_RELATIVE,
};
use crate::journal::{FsJournal, PlanWriter};
use crate::lock_file;
use crate::ops_install;
use crate::ownership;
use crate::ports::{
    acquire_exclusive, acquire_shared, Clock, DirEntryFacts, ExclusiveGuard, FileKind,
    HistoryAccess, OpContext, PlanStatus, Runtime, ScopeFs, ScopedReads,
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
    /// Phase 1: runtime harness detection.
    Harnesses,
    /// Phase 2: propose a frontmatter fix.
    PreviewFrontmatterRepair,
    /// Phase 2: apply a proposed fix.
    ApplyFrontmatterRepair,
    /// Phase 2: read history.
    ListEvents,
    /// Phase 2: revert one event.
    RestoreEvent,
    /// Phase 3: move a universal deployment to the parked root.
    Park,
    /// Phase 3: move a parked deployment back to the universal root.
    Unpark,
    /// Phase 3: flip a skill's native per-harness switch.
    SetHarnessEnabled,
    /// Group 3: run the doctor invariants for one skill and repair whatever
    /// it can.
    FixSkill,
    /// Group 3: find differing copies of a skill without merging them.
    DiagnoseConflict,
    /// Unit 3.9: take a mutable deployment off disk.
    Remove,
    /// Group 3: refresh one already-installed skill in place.
    Update,
    /// Group 3: refresh a batch of already-installed skills in place.
    UpdateAll,
    /// Group 3: put one skill on disk by `Copy`, `Dotagents`, or `SkillsSh`.
    Install,
    /// Group 3: read the saved or defaulted install method/harnesses.
    InstallPreferences,
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
impl Outcome for HarnessReport {}
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
impl Outcome for ParkOutcome {
    fn event_id(&self) -> Option<EventId> {
        Some(self.event_id.clone())
    }
}
impl Outcome for UnparkOutcome {
    fn event_id(&self) -> Option<EventId> {
        Some(self.event_id.clone())
    }
}
impl Outcome for SetHarnessEnabledOutcome {
    fn event_id(&self) -> Option<EventId> {
        Some(self.event_id.clone())
    }
}
impl Outcome for crate::dto::FixSkillOutcome {
    fn found_issues(&self) -> bool {
        !self.unrepaired.is_empty() || !self.conflicts.is_empty()
    }

    fn event_id(&self) -> Option<EventId> {
        // A fix can apply several repairs, each its own event; the envelope
        // names only the last one it wrote, matching the "what this call
        // itself created" convention every other outcome follows.
        self.applied.last().map(|applied| {
            let crate::dto::FixApplied::FrontmatterRepair { event_id, .. } = applied;
            event_id.clone()
        })
    }
}
impl Outcome for crate::dto::ConflictReport {
    fn found_issues(&self) -> bool {
        !self.conflicts.is_empty()
    }
}
impl Outcome for crate::dto::RemoveOutcome {
    fn event_id(&self) -> Option<EventId> {
        Some(self.event_id.clone())
    }
}
impl Outcome for crate::dto::UpdateOutcome {
    fn event_id(&self) -> Option<EventId> {
        Some(self.event_id.clone())
    }
}
impl Outcome for crate::dto::UpdateAllOutcome {
    fn status(&self) -> OpStatus {
        if self.errors.is_empty() {
            OpStatus::Ok
        } else if self.items.iter().any(|i| i.outcome.is_some()) {
            OpStatus::Partial
        } else {
            OpStatus::Error
        }
    }

    fn found_issues(&self) -> bool {
        !self.errors.is_empty()
    }
}
impl Outcome for crate::dto::InstallOutcome {
    fn event_id(&self) -> Option<EventId> {
        match self {
            // `NeedsTrust` wrote nothing, so it records no event.
            crate::dto::InstallOutcome::Installed { event_id, .. } => Some(event_id.clone()),
            crate::dto::InstallOutcome::NeedsTrust { .. } => None,
        }
    }
}
impl Outcome for crate::dto::InstallPreferences {}

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
    /// This call's timing, taken from `ctx` at envelope-building time.
    /// `None` only when the call failed before an op function ran at all (a
    /// scope that failed to normalize), since every op that runs records its
    /// own timing before returning.
    pub timings: Option<crate::timing::OpTiming>,
}

impl<T: Outcome> ResultEnvelope<T> {
    /// Wraps an operation result, taking `ctx`'s filed timing along with it.
    pub fn from_result(
        operation: Operation,
        scope: &NormalizedScope,
        ctx: &OpContext,
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
            correlation_id: ctx.correlation_id.clone(),
            event_id,
            timings: ctx.take_timing(),
        }
    }
}

impl<T: Outcome> ResultEnvelope<T> {
    /// Process exit status: `0` ok, `1` ok with issues, `4` partial, else
    /// the first error's code. Partial wins over issues.
    pub fn exit_status(&self) -> i32 {
        match self.status {
            OpStatus::Ok if self.data.as_ref().is_some_and(Outcome::found_issues) => 1,
            OpStatus::Ok => 0,
            OpStatus::Partial => ErrorCode::Incomplete.exit_status(),
            OpStatus::Error => self.errors.first().map_or(1, |e| e.code.exit_status()),
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
    let clock = rt.ports.clock.as_ref();
    let mut op_steps = Vec::new();
    let budget = rt.scope.raw.read_timeout();
    let fs = rt.ports.fs.as_ref();
    let home = &rt.scope.home.lexical;

    let step_start = clock.monotonic();
    let disable_sources = DisableSources::read(
        fs,
        home,
        rt.scope.raw.opencode_config_root.as_deref(),
        &rt.scope.codex_home,
    );

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
    op_steps.push(crate::timing::step(clock, "ledgers_read", step_start));

    let timings = ScanTimings::default();
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
        timings: &timings,
    };
    let mut accum = ScanAccum {
        skills: BTreeMap::new(),
        observations: Vec::new(),
        unread_roots: Vec::new(),
        completeness: Completeness::Complete,
        // Deployment id -> canonical directory, filled in by
        // `process_entries` and consumed by
        // `propagate_verified_linked_owners` once every root has been
        // walked.
        resolved_paths: HashMap::new(),
        content_cache: HashMap::new(),
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

    // Global before project, in all four loops, is the read-budget invariant
    // the module doc promises.
    let step_start = clock.monotonic();
    for target in &global_targets {
        scan_one_target(&sc, target, &mut accum)?;
    }
    for target in &global_plugin_targets {
        scan_one_plugin_target(&sc, target, &mut accum)?;
    }
    for target in &project_targets {
        scan_one_target(&sc, target, &mut accum)?;
    }
    for target in &project_plugin_targets {
        scan_one_plugin_target(&sc, target, &mut accum)?;
    }
    op_steps.push(crate::timing::step(clock, "roots_walk", step_start));
    op_steps.push(crate::timing::StepTiming {
        name: "dir_walk".to_string(),
        elapsed_ms: timings.dir_walk.get().as_millis() as u64,
    });
    op_steps.push(crate::timing::StepTiming {
        name: "skill_md_read".to_string(),
        elapsed_ms: timings.skill_md_read.get().as_millis() as u64,
    });
    op_steps.push(crate::timing::StepTiming {
        name: "frontmatter_parse".to_string(),
        elapsed_ms: timings.frontmatter_parse.get().as_millis() as u64,
    });
    op_steps.push(crate::timing::StepTiming {
        name: "plugin_cache_walk".to_string(),
        elapsed_ms: timings.plugin_cache_walk.get().as_millis() as u64,
    });

    let ScanAccum {
        mut skills,
        observations,
        unread_roots,
        completeness,
        resolved_paths,
        content_cache: _,
    } = accum;

    // Every root has been walked, so every canonical universal deployment
    // any link could point to now has a `resolved_paths` entry: assign
    // verified links their canonical owner.
    let step_start = clock.monotonic();
    for skill in skills.values_mut() {
        propagate_verified_linked_owners(skill, &resolved_paths);
    }
    op_steps.push(crate::timing::step(
        clock,
        "link_owner_propagation",
        step_start,
    ));

    // A universal skill Claude Code has no per-skill link for is one that
    // reader has disabled: Claude Code only ever reads a universal skill
    // through an explicit `~/.claude/skills/<name>` link, never the shared
    // root directly (`reads_universal_root: Support::No` in `harness.rs`).
    let step_start = clock.monotonic();
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
    op_steps.push(crate::timing::step(
        clock,
        "claude_universal_reader_check",
        step_start,
    ));

    let skills: Vec<InstalledSkillDto> = skills.into_values().collect();

    ctx.record_timing(crate::timing::op_timing(clock, "scan", start, op_steps));

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
        unread_roots,
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
    timings: &'a ScanTimings,
}

/// Per-section durations accumulated across every [`scan_one_target`] and
/// [`scan_one_plugin_target`] call in one [`scan_inner`] run, filed as
/// `scan`'s steps alongside `roots_walk` (the one step spanning all four
/// loops these are measured inside of).
#[derive(Default)]
struct ScanTimings {
    dir_walk: Cell<Duration>,
    skill_md_read: Cell<Duration>,
    frontmatter_parse: Cell<Duration>,
    plugin_cache_walk: Cell<Duration>,
}

impl ScanTimings {
    fn add(cell: &Cell<Duration>, elapsed: Duration) {
        cell.set(cell.get() + elapsed);
    }
}

/// The `scan_inner` accumulators every target folds into, in target order.
struct ScanAccum {
    skills: BTreeMap<String, InstalledSkillDto>,
    observations: Vec<Observation>,
    /// Root paths this run could not read at all - see [`Inventory::unread_roots`].
    unread_roots: Vec<PathBuf>,
    completeness: Completeness,
    /// Deployment id -> canonical directory, filled in by `process_entries`
    /// and consumed by `propagate_verified_linked_owners` once every root
    /// has been walked.
    resolved_paths: HashMap<DeploymentId, PathBuf>,
    /// Canonical skill directory -> its already-read `SKILL.md` facts,
    /// filled in and consumed by `process_entries` across every target: a
    /// universal skill is listed once under the shared root (its canonical
    /// entry) and once more under the one harness root it's linked from,
    /// and both listings resolve to the same canonical directory. Without
    /// this, the second listing would read the same `SKILL.md` again.
    content_cache: HashMap<PathBuf, CachedSkillRead>,
}

/// One canonical skill directory's already-computed `SKILL.md` read,
/// cached by [`process_entries`] so a second directory entry resolving to
/// the same canonical path (a per-skill symlink into the universal root)
/// reuses it instead of reading and walking the folder again.
#[derive(Clone)]
struct CachedSkillRead {
    description: Option<String>,
    violations: Vec<String>,
    truncated: bool,
    facts: ContentFacts,
}

/// Reads one [`ScanTarget`] (and its [`MOVE_ASIDE_DIR_NAME`] holding
/// directory) into `accum`, or records a budget/read-error observation.
/// The body of `scan_inner`'s former `for target in scan_targets(rt)` loop.
fn scan_one_target(
    sc: &ScanCtx,
    target: &ScanTarget,
    accum: &mut ScanAccum,
) -> Result<(), CoreError> {
    sc.ctx.checkpoint()?;
    if sc.rt.ports.clock.monotonic().saturating_sub(sc.start) > sc.budget {
        accum.completeness = Completeness::Partial;
        accum.observations.push(Observation {
            root: RootRef::new(target.scope.clone(), target.kind.clone()).ok(),
            message: "read budget exceeded before this root could be scanned".to_string(),
        });
        accum.unread_roots.push(target.path.clone());
        return Ok(());
    }

    // A root whose lexical path is itself a symlink (e.g. `~/.claude/
    // skills -> ../.agents/skills`) shares every deployment under it
    // through that one link, not per skill.
    let whole_dir_link = matches!(
        sc.fs.symlink_metadata(&target.path).map(|m| m.kind),
        Ok(FileKind::Symlink)
    );

    // A cheap existence check before `read_dir`: the catalog names far more
    // roots (every harness's global and project root, fanned across every
    // tracked project) than a given home actually has on disk, so most
    // targets - and almost every `MOVE_ASIDE_DIR_NAME` holding directory -
    // don't exist. `symlink_metadata` isn't bounded by the scan's
    // `read_dir`-per-directory budget the way `read_dir` itself is, so
    // ruling out a missing root this way, rather than by calling `read_dir`
    // and matching its `NotFound`, is a real directory listing saved, not
    // just the same cost moved elsewhere.
    if !root_dir_missing(sc.fs, &target.path) {
        match timed_read_root_entries(sc, &target.path) {
            Ok(names) => process_entries(
                &EntryContext {
                    fs: sc.fs,
                    ctx: sc.ctx,
                    clock: sc.rt.ports.clock.as_ref(),
                    scope: &sc.rt.scope,
                    home: sc.home,
                    disable_sources: sc.disable_sources,
                    scope_ledgers: sc.scope_ledgers,
                    home_registry: sc.home_registry,
                    target,
                    base_dir: &target.path,
                    whole_dir_link,
                    forced_disabled_by: None,
                    timings: sc.timings,
                },
                &names,
                sc.req,
                accum,
            )?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                accum.completeness = Completeness::Partial;
                accum.observations.push(Observation {
                    root: RootRef::new(target.scope.clone(), target.kind.clone()).ok(),
                    message: format!("could not read root: {e}"),
                });
                accum.unread_roots.push(target.path.clone());
            }
        }
    }

    scan_move_aside_dir(sc, target, whole_dir_link, accum)
}

/// True when `path` cannot be listed because nothing is there:
/// [`ScopeFs::symlink_metadata`] reports [`std::io::ErrorKind::NotFound`].
/// Any other outcome (it exists, or some other error) defers to the real
/// `read_dir` call so that call's own error handling still applies.
fn root_dir_missing(fs: &dyn ScopeFs, path: &Path) -> bool {
    matches!(
        fs.symlink_metadata(path),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound
    )
}

/// Reads `target.path`'s [`MOVE_ASIDE_DIR_NAME`] holding directory into
/// `accum`, when it exists. Skills Skill Studio moved aside stay
/// deployments (so the UI can still show and un-park them), just disabled.
/// Split out of [`scan_one_target`] so the caller can skip it too, via the
/// same [`root_dir_missing`] check, when the target root itself doesn't
/// exist - a root that isn't there never holds a move-aside directory
/// either.
fn scan_move_aside_dir(
    sc: &ScanCtx,
    target: &ScanTarget,
    whole_dir_link: bool,
    accum: &mut ScanAccum,
) -> Result<(), CoreError> {
    let move_aside_dir = target.path.join(MOVE_ASIDE_DIR_NAME);
    if root_dir_missing(sc.fs, &move_aside_dir) {
        return Ok(());
    }
    if let Ok(names) = timed_read_root_entries(sc, &move_aside_dir) {
        process_entries(
            &EntryContext {
                fs: sc.fs,
                ctx: sc.ctx,
                clock: sc.rt.ports.clock.as_ref(),
                scope: &sc.rt.scope,
                home: sc.home,
                disable_sources: sc.disable_sources,
                scope_ledgers: sc.scope_ledgers,
                home_registry: sc.home_registry,
                target,
                base_dir: &move_aside_dir,
                whole_dir_link,
                forced_disabled_by: Some(DisabledBy::StudioMoved),
                timings: sc.timings,
            },
            &names,
            sc.req,
            accum,
        )?;
    }
    Ok(())
}

/// Reads one [`PluginCacheTarget`] into `accum`, or records a budget
/// observation. The body of `scan_inner`'s former
/// `for target in plugin_scan_targets(rt)` loop.
fn scan_one_plugin_target(
    sc: &ScanCtx,
    target: &PluginCacheTarget,
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
        accum.unread_roots.push(target.path.clone());
        return Ok(());
    }
    // Same existence pre-check as `scan_one_target`: most plugin cache
    // roots the catalog names don't exist on a given home, and skipping
    // straight to `enumerate_plugin_skills`'s `read_dir` would otherwise
    // spend it on a directory nothing is in.
    if root_dir_missing(sc.fs, &target.path) {
        return Ok(());
    }
    let walk_start = sc.rt.ports.clock.monotonic();
    let plugin_skills = enumerate_plugin_skills(sc.fs, target);
    ScanTimings::add(
        &sc.timings.plugin_cache_walk,
        sc.rt.ports.clock.monotonic().saturating_sub(walk_start),
    );
    for plugin_skill in plugin_skills {
        if !sc.req.skills.is_empty() && !sc.req.skills.iter().any(|s| s.0 == plugin_skill.name) {
            continue;
        }
        match read_skill_md(
            sc.fs,
            sc.ctx,
            sc.rt.ports.clock.as_ref(),
            &plugin_skill.skill_dir,
            &plugin_skill.name,
            sc.timings,
        )? {
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
                let content_fingerprint = Some(facts.content_fingerprint.clone());
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
                // The root itself was read fine; only this one skill
                // directory was unreadable, so scope the carry-over to it
                // rather than the whole plugin cache root.
                accum.unread_roots.push(plugin_skill.skill_dir.clone());
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
/// As [`read_root_entries`], accumulating the read's duration onto
/// `sc.timings.dir_walk` (the scan step this call is part of).
fn timed_read_root_entries(sc: &ScanCtx, dir: &Path) -> std::io::Result<Vec<DirEntryFacts>> {
    let start = sc.rt.ports.clock.monotonic();
    let result = read_root_entries(sc.fs, dir);
    ScanTimings::add(
        &sc.timings.dir_walk,
        sc.rt.ports.clock.monotonic().saturating_sub(start),
    );
    result
}

fn read_root_entries(fs: &dyn ScopeFs, dir: &Path) -> std::io::Result<Vec<DirEntryFacts>> {
    let entries = fs.read_dir(dir)?;
    let mut names: Vec<_> = entries
        .into_iter()
        .filter(crate::ports::is_skill_shaped_entry)
        .collect();
    names.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(names)
}

/// Everything [`process_entries`] needs that stays the same across every
/// entry in one call: which root, which reader, which sources of truth.
struct EntryContext<'a> {
    fs: &'a dyn ScopeFs,
    ctx: &'a OpContext,
    clock: &'a dyn Clock,
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
    /// The same accumulator [`ScanCtx::timings`] points at, so
    /// [`read_skill_md`] can file `skill_md_read`/`frontmatter_parse` time
    /// here too.
    timings: &'a ScanTimings,
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
    accum: &mut ScanAccum,
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

        // A non-link entry's own canonicalized path, computed once and
        // reused below for `canonical_key`, `resolved_path`, and
        // `dto_resolved_path` instead of canonicalizing `skill_dir` again
        // for each.
        let own_canonical = (!is_link)
            .then(|| cx.fs.canonicalize(&skill_dir).ok())
            .flatten();
        // A per-skill symlink into the universal root and its canonical
        // universal entry are two directory listings of the one real
        // folder: keyed by canonical path so the second listing reuses the
        // first's `SKILL.md` read and folder walk instead of repeating
        // them.
        let canonical_key = if is_link {
            canonical.clone()
        } else {
            own_canonical.clone()
        };

        let (description, violations, content_fingerprint, facts) = if unresolved_link {
            (None, Vec::new(), None, Box::new(ContentFacts::default()))
        } else if let Some(cached) = canonical_key
            .as_ref()
            .and_then(|key| accum.content_cache.get(key))
        {
            if cached.truncated {
                if let Some(observation) = truncated_skill_md_observation(
                    cx.target.scope.clone(),
                    cx.target.kind.clone(),
                    &entry.name,
                ) {
                    accum.observations.push(observation);
                }
            }
            (
                cached.description.clone(),
                cached.violations.clone(),
                Some(cached.facts.content_fingerprint.clone()),
                Box::new(cached.facts.clone()),
            )
        } else {
            match read_skill_md(cx.fs, cx.ctx, cx.clock, &skill_dir, &entry.name, cx.timings)? {
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
                            accum.observations.push(observation);
                        }
                    }
                    if let Some(key) = canonical_key.clone() {
                        accum.content_cache.insert(
                            key,
                            CachedSkillRead {
                                description: description.clone(),
                                violations: violations.clone(),
                                truncated,
                                facts: (*facts).clone(),
                            },
                        );
                    }
                    (
                        description,
                        violations,
                        Some(facts.content_fingerprint.clone()),
                        facts,
                    )
                }
                SkillMdRead::NotASkill => continue,
                SkillMdRead::Unreadable(message) => {
                    accum.completeness = Completeness::Partial;
                    accum.observations.push(Observation {
                        root: RootRef::new(cx.target.scope.clone(), cx.target.kind.clone()).ok(),
                        message,
                    });
                    // The root itself was read fine; only this one skill
                    // directory was unreadable, so scope the carry-over to
                    // it rather than the whole root.
                    accum.unread_roots.push(skill_dir.clone());
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
            Some(own_canonical.clone().unwrap_or_else(|| skill_dir.clone()))
        };
        if let Some(resolved_path) = resolved_path {
            accum.resolved_paths.insert(id.clone(), resolved_path);
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
            own_canonical.clone().filter(|c| c != &skill_dir)
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

        insert_deployment(&mut accum.skills, &entry.name, description, deployment);
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
    clock: &dyn Clock,
    skill_dir: &Path,
    name: &str,
    timings: &ScanTimings,
) -> Result<SkillMdRead, CoreError> {
    let skill_md = skill_dir.join("SKILL.md");
    let read_start = clock.monotonic();
    let read_result = fs.read_prefix(&skill_md, SKILL_MD_MAX_BYTES);
    ScanTimings::add(
        &timings.skill_md_read,
        clock.monotonic().saturating_sub(read_start),
    );
    let (bytes, truncated) = match read_result {
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
    let parse_start = clock.monotonic();
    let parsed = frontmatter::parse_frontmatter(&content);
    ScanTimings::add(
        &timings.frontmatter_parse,
        clock.monotonic().saturating_sub(parse_start),
    );
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

/// Reports whether `plugin_dir` holds one of [`PLUGIN_MANIFEST_CANDIDATES`],
/// leniently: a manifest that exists but fails to parse as JSON, or parses
/// without a `name`, still counts as "a plugin is here" rather than failing
/// the walk. Only presence matters to the caller; the manifest's own fields
/// (`name`, `version`) come from the path components in
/// [`enumerate_plugin_skills`] instead.
fn plugin_manifest_present(fs: &dyn ScopeFs, plugin_dir: &Path) -> bool {
    PLUGIN_MANIFEST_CANDIDATES.iter().any(|candidate| {
        fs.read_capped(&plugin_dir.join(candidate), SKILL_MD_MAX_BYTES)
            .is_ok()
    })
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
        if plugin_manifest_present(fs, &path) {
            found.push(path);
            continue;
        }
        if depth_remaining > 0 {
            walk_for_plugin_roots(fs, &path, depth_remaining - 1, found);
        }
    }
}

/// Codex and `OpenCode`'s own per-skill disable switches, read once per
/// `scan` call (they are global config files, not per-root).
struct DisableSources {
    /// Canonical `SKILL.md` paths Codex's `[[skills.config]] enabled =
    /// false` rows name. Mirrors `codex_skill_config.rs`
    /// `read_disabled_skill_md_paths`.
    codex_disabled_skill_md: Vec<PathBuf>,
    /// `permission.skill` (v1) and `permissions[]` (v2) skill rules
    /// `opencode.json` holds, from [`crate::opencode_config::read_skill_rules`]
    /// - the same read the write path and every adapter use, so a scan and
    ///   a deny write always agree on what "denied" means.
    opencode_skill_rules: crate::opencode_config::OpencodeSkillRules,
    /// Claude Code `settings.json` `enabledPlugins["<plugin>@<marketplace>"]`,
    /// keyed by that same `<plugin>@<marketplace>` id.
    claude_enabled_plugins: HashMap<String, bool>,
}

impl DisableSources {
    fn read(
        fs: &dyn ScopeFs,
        home: &Path,
        opencode_config_root: Option<&Path>,
        codex_home: &Path,
    ) -> Self {
        let opencode_config_dir = opencode_config_root
            .map_or_else(|| home.join(".config").join("opencode"), Path::to_path_buf);
        DisableSources {
            codex_disabled_skill_md: read_codex_disabled_skill_md_paths(fs, codex_home),
            opencode_skill_rules: crate::opencode_config::read_skill_rules(
                fs,
                &opencode_config_dir,
            ),
            claude_enabled_plugins: read_claude_enabled_plugins(fs, home),
        }
    }
}

fn read_codex_disabled_skill_md_paths(fs: &dyn ScopeFs, codex_home: &Path) -> Vec<PathBuf> {
    let path = codex_home.join("config.toml");
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

/// `<codex_home>/config.toml`.
fn codex_config_path(codex_home: &Path) -> PathBuf {
    codex_home.join("config.toml")
}

/// Reads `<codex_home>/config.toml` as a format-preserving `toml_edit`
/// document. A missing file parses as an empty document (there is nothing to
/// preserve); a file that fails to parse is an error, since a write from
/// here would otherwise silently discard whatever the user had in it.
fn read_codex_config_document(
    fs: &dyn ScopeFs,
    codex_home: &Path,
) -> Result<toml_edit::DocumentMut, CoreError> {
    let path = codex_config_path(codex_home);
    let text = match fs.read_capped(&path, SKILL_MD_MAX_BYTES) {
        Ok(bytes) => String::from_utf8(bytes).map_err(|e| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                format!("{} is not valid UTF-8: {e}", path.display()),
            )
            .at(&path)
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(CoreError::io(&path, e)),
    };
    text.parse::<toml_edit::DocumentMut>().map_err(|e| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            format!("{} is not valid TOML: {e}", path.display()),
        )
        .at(&path)
    })
}

/// Iterates `[[skills.config]]` rows in a `toml_edit` document, tolerating a
/// document with no `skills` table, no `config` array, or a `config` that
/// isn't an array of tables.
fn codex_skills_config_rows(
    doc: &toml_edit::DocumentMut,
) -> impl Iterator<Item = &toml_edit::Table> {
    doc.get("skills")
        .and_then(toml_edit::Item::as_table)
        .and_then(|t| t.get("config"))
        .and_then(toml_edit::Item::as_array_of_tables)
        .into_iter()
        .flatten()
}

/// Index of the `[[skills.config]]` row whose `path` matches `skill_md_path`,
/// if any.
fn codex_find_row_index(doc: &toml_edit::DocumentMut, skill_md_path: &Path) -> Option<usize> {
    let target = skill_md_path.to_string_lossy();
    codex_skills_config_rows(doc)
        .position(|row| row.get("path").and_then(toml_edit::Item::as_str) == Some(target.as_ref()))
}

/// Converts a removed table header's surrounding text into text that can sit
/// before the next table header. `toml_edit` writes the header's suffix
/// before adding its own newline, so the newline must move with a non-empty
/// suffix.
fn codex_table_decor_as_prefix(table: &toml_edit::Table) -> String {
    let prefix = table
        .decor()
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .unwrap_or_default();
    let suffix = table
        .decor()
        .suffix()
        .and_then(|suffix| suffix.as_str())
        .unwrap_or_default();
    if !prefix.contains('#') && !suffix.contains('#') {
        return String::new();
    }
    let mut text = prefix.to_string();
    if !suffix.is_empty() {
        text.push_str(suffix);
        if !suffix.ends_with('\n') {
            text.push('\n');
        }
    }
    text
}

fn codex_prepend_table_decor(table: &mut toml_edit::Table, text: &str) {
    if text.is_empty() {
        return;
    }
    let existing = table
        .decor()
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .unwrap_or_default()
        .to_string();
    table.decor_mut().set_prefix(format!("{text}{existing}"));
}

struct CodexOrphanedTableDecor {
    position: Option<isize>,
    text: String,
}

fn codex_next_table_position(table: &toml_edit::Table, removed_position: isize) -> Option<isize> {
    let mut next = table
        .position()
        .filter(|position| *position > removed_position);
    for (_, item) in table {
        let child_next = match item {
            toml_edit::Item::Table(child) => codex_next_table_position(child, removed_position),
            toml_edit::Item::ArrayOfTables(array) => array
                .iter()
                .filter_map(|child| codex_next_table_position(child, removed_position))
                .min(),
            _ => None,
        };
        next = next.into_iter().chain(child_next).min();
    }
    next
}

fn codex_table_at_position_mut(
    table: &mut toml_edit::Table,
    position: isize,
) -> Option<&mut toml_edit::Table> {
    if table.position() == Some(position) {
        return Some(table);
    }
    for (_, item) in table.iter_mut() {
        let found = match item {
            toml_edit::Item::Table(child) => codex_table_at_position_mut(child, position),
            toml_edit::Item::ArrayOfTables(array) => array
                .iter_mut()
                .find_map(|child| codex_table_at_position_mut(child, position)),
            _ => None,
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Moves decor orphaned by a removed table to the next table in document
/// order, or to the document trailing text when no table follows it.
fn codex_rehome_table_decor(
    doc: &mut toml_edit::DocumentMut,
    removed_position: Option<isize>,
    text: &str,
) {
    if text.is_empty() {
        return;
    }
    if let Some(next_position) =
        removed_position.and_then(|position| codex_next_table_position(doc.as_table(), position))
    {
        let Some(next) = codex_table_at_position_mut(doc.as_table_mut(), next_position) else {
            unreachable!("codex_next_table_position returned an existing table");
        };
        codex_prepend_table_decor(next, text);
        return;
    }
    let trailing = doc.trailing().as_str().unwrap_or_default();
    doc.set_trailing(format!("{text}{trailing}"));
}

fn codex_rehome_table_decor_blocks(
    doc: &mut toml_edit::DocumentMut,
    mut blocks: Vec<CodexOrphanedTableDecor>,
) {
    blocks.retain(|block| !block.text.is_empty());
    blocks.sort_by_key(|block| std::cmp::Reverse(block.position));
    for block in blocks {
        codex_rehome_table_decor(doc, block.position, &block.text);
    }
}

/// Adds (or removes) a `[[skills.config]] path = "<skill_md_path>" enabled =
/// false` row so Codex disables (or stops disabling) that skill, preserving
/// every other byte of `<codex_home>/config.toml` - other tables, comments,
/// and formatting survive because this edits the parsed `DocumentMut` in
/// place rather than re-serializing a plain value. Ported from the desktop's
/// former `codex_skill_config.rs::set_skill_disabled`, onto `ScopeFs` and an
/// `ExclusiveGuard` instead of raw `std::fs`. Idempotent: disabling an
/// already-disabled row, or enabling one that isn't disabled, is a no-op
/// write.
pub fn set_codex_skill_disabled(
    rt: &Runtime,
    ctx: &OpContext,
    skill_md_path: &Path,
    disabled: bool,
) -> Result<(), CoreError> {
    let guard = acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope)?;
    set_codex_skill_disabled_with(rt, ctx, &guard, skill_md_path, disabled)
}

/// The lease-holding half of [`set_codex_skill_disabled`]. The caller
/// already holds the root's exclusive lease - the desktop command's
/// `WriteLease`, or a `MutationSession` - so this must not acquire a second
/// one; advisory locks do not nest in-process, and a second `acquire_exclusive`
/// on the same root self-deadlocks until the lease times out.
pub fn set_codex_skill_disabled_with(
    rt: &Runtime,
    ctx: &OpContext,
    guard: &ExclusiveGuard,
    skill_md_path: &Path,
    disabled: bool,
) -> Result<(), CoreError> {
    ctx.checkpoint()?;
    let fs = rt.ports.fs.as_ref();
    let codex_home = &rt.scope.codex_home;
    let mut doc = read_codex_config_document(fs, codex_home)?;
    codex_write_disabled_row(&mut doc, skill_md_path, disabled)
        .map_err(|e| e.at(codex_config_path(codex_home)))?;
    codex_write_config_document(rt, fs, guard, codex_home, &doc)
}

/// The in-memory half of [`set_codex_skill_disabled`], split out so
/// [`codex_rewrite_skill_path`] can reuse the row lookup and decor-rehoming
/// without re-deriving them. Errors rather than panics when `skills` or
/// `skills.config` already exists in `config.toml` under a type the user
/// wrote there themselves - a string, an inline table, and so on - that a
/// disable row can't be inserted into.
fn codex_write_disabled_row(
    doc: &mut toml_edit::DocumentMut,
    skill_md_path: &Path,
    disabled: bool,
) -> Result<(), CoreError> {
    let existing = codex_find_row_index(doc, skill_md_path);

    if !disabled {
        if let Some(idx) = existing {
            let (removed_decor, array_is_empty) = {
                let Some(array) = doc["skills"]["config"].as_array_of_tables_mut() else {
                    unreachable!(
                        "codex_find_row_index only returns Some when this is an array of tables"
                    );
                };
                let removed = array.remove(idx);
                let removed_decor = CodexOrphanedTableDecor {
                    position: removed.position(),
                    text: codex_table_decor_as_prefix(&removed),
                };
                if let Some(next_row) = array.get_mut(idx) {
                    codex_prepend_table_decor(next_row, &removed_decor.text);
                    (None, false)
                } else {
                    (Some(removed_decor), array.is_empty())
                }
            };

            let mut orphaned_decor = removed_decor.into_iter().collect::<Vec<_>>();
            let remove_skills = {
                let Some(skills_table) = doc["skills"].as_table_mut() else {
                    unreachable!("skills is a table when config was");
                };
                if array_is_empty {
                    skills_table.remove("config");
                }
                if skills_table.is_empty() {
                    orphaned_decor.push(CodexOrphanedTableDecor {
                        position: skills_table.position(),
                        text: codex_table_decor_as_prefix(skills_table),
                    });
                    true
                } else {
                    false
                }
            };
            if remove_skills {
                doc.as_table_mut().remove("skills");
            }
            codex_rehome_table_decor_blocks(doc, orphaned_decor);
        }
    } else if existing.is_none() {
        let skills_item = doc
            .entry("skills")
            .or_insert_with(|| toml_edit::Item::Table(toml_edit::Table::new()));
        let skills_type = skills_item.type_name();
        let skills_table = skills_item.as_table_mut().ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                format!("skills in config.toml is a {skills_type}, not a table"),
            )
        })?;
        let config_item = skills_table
            .entry("config")
            .or_insert_with(|| toml_edit::Item::ArrayOfTables(Default::default()));
        let config_type = config_item.type_name();
        let config_array = config_item.as_array_of_tables_mut().ok_or_else(|| {
            CoreError::new(
                ErrorCode::InvalidRequest,
                format!("skills.config in config.toml is a {config_type}, not an array of tables"),
            )
        })?;
        let mut row = toml_edit::Table::new();
        row["path"] = toml_edit::value(skill_md_path.to_string_lossy().to_string());
        row["enabled"] = toml_edit::value(false);
        config_array.push(row);
    }
    // `existing.is_some() && disabled`: already disabled, nothing to do -
    // idempotent by construction.
    Ok(())
}

/// Rewrites an existing `[[skills.config]]` row's `path` from
/// `old_skill_md` to `new_skill_md`, leaving `enabled` and every other byte
/// untouched. A no-op when no row names `old_skill_md` - not every skill
/// Codex knows about has been explicitly disabled.
///
/// `ops::park` and `ops::unpark` call this after moving a deployment's
/// folder, so a skill that was disabled through Codex's own config stays
/// disabled at its new path instead of leaving a stale row that no longer
/// matches anything on disk (the bug `docs/action-map/harnesses/codex.md`
/// names).
pub fn codex_rewrite_skill_path(
    rt: &Runtime,
    ctx: &OpContext,
    guard: &ExclusiveGuard,
    old_skill_md: &Path,
    new_skill_md: &Path,
) -> Result<(), CoreError> {
    ctx.checkpoint()?;
    let fs = rt.ports.fs.as_ref();
    let codex_home = &rt.scope.codex_home;
    let mut doc = read_codex_config_document(fs, codex_home)?;
    let Some(idx) = codex_find_row_index(&doc, old_skill_md) else {
        return Ok(());
    };
    let Some(rows) = doc["skills"]["config"].as_array_of_tables_mut() else {
        unreachable!("codex_find_row_index only returns Some when this is an array of tables");
    };
    let Some(row) = rows.get_mut(idx) else {
        unreachable!("codex_find_row_index returned a valid index");
    };
    row["path"] = toml_edit::value(new_skill_md.to_string_lossy().to_string());
    codex_write_config_document(rt, fs, guard, codex_home, &doc)
}

fn codex_write_config_document(
    rt: &Runtime,
    fs: &dyn ScopeFs,
    guard: &ExclusiveGuard,
    codex_home: &Path,
    doc: &toml_edit::DocumentMut,
) -> Result<(), CoreError> {
    let path = codex_config_path(codex_home);
    let Some(parent) = path.parent() else {
        unreachable!("config.toml always has a parent");
    };
    let parent = parent.to_path_buf();
    let scoped_parent = crate::ports::confine(&rt.scope, fs, &parent)?;
    fs.create_dir_all(guard, &scoped_parent)
        .map_err(|e| CoreError::io(&parent, e))?;
    let scoped_path = crate::ports::confine(&rt.scope, fs, &path)?;
    fs.write_atomic(guard, &scoped_path, doc.to_string().as_bytes())
        .map_err(|e| CoreError::io(&path, e))
}

/// `<skill_dir>/agents/openai.yaml` - Codex's own invocation-policy sidecar,
/// next to `SKILL.md`.
fn codex_openai_yaml_path(skill_dir: &Path) -> PathBuf {
    skill_dir.join("agents").join("openai.yaml")
}

/// Sets or clears `policy.allow_implicit_invocation: false` in a Codex
/// deployment's `agents/openai.yaml`, preserving any other top-level keys.
/// Creates the file (and its `agents/` directory) when setting the key on a
/// skill that didn't have one; deletes the file entirely when clearing the
/// key leaves it empty, rather than leaving a stray `{}`. Ported from the
/// desktop's former `skill_invocation.rs::patch_codex_openai_yaml`, onto
/// `ScopeFs` and an `ExclusiveGuard`.
pub fn set_codex_sidecar_implicit_invocation(
    rt: &Runtime,
    ctx: &OpContext,
    skill_dir: &Path,
    user_only: bool,
) -> Result<(), CoreError> {
    ctx.checkpoint()?;
    let fs = rt.ports.fs.as_ref();
    let guard = acquire_exclusive(rt.ports.leases.as_ref(), &rt.scope)?;
    let path = codex_openai_yaml_path(skill_dir);
    let mut root: serde_yaml::Mapping = match fs.read_capped(&path, SKILL_MD_MAX_BYTES) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes).map_err(|e| {
                CoreError::new(
                    ErrorCode::InvalidRequest,
                    format!("{} is not valid UTF-8: {e}", path.display()),
                )
                .at(&path)
            })?;
            match serde_yaml::from_str(&text) {
                Ok(serde_yaml::Value::Mapping(m)) => m,
                Ok(_) | Err(_) => {
                    return Err(CoreError::new(
                        ErrorCode::InvalidRequest,
                        format!("{} is not a YAML mapping", path.display()),
                    )
                    .at(&path));
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => serde_yaml::Mapping::new(),
        Err(e) => return Err(CoreError::io(&path, e)),
    };

    let policy_key = serde_yaml::Value::String("policy".to_string());
    let allow_key = serde_yaml::Value::String("allow_implicit_invocation".to_string());
    let mut policy = match root.get(&policy_key) {
        Some(serde_yaml::Value::Mapping(m)) => m.clone(),
        _ => serde_yaml::Mapping::new(),
    };

    if user_only {
        policy.insert(allow_key, serde_yaml::Value::Bool(false));
        root.insert(policy_key, serde_yaml::Value::Mapping(policy));
    } else {
        policy.remove(&allow_key);
        if policy.is_empty() {
            root.remove(&policy_key);
        } else {
            root.insert(policy_key, serde_yaml::Value::Mapping(policy));
        }
        if root.is_empty() {
            if fs.symlink_metadata(&path).is_ok() {
                let scoped_path = crate::ports::confine(&rt.scope, fs, &path)?;
                fs.remove_file(&guard, &scoped_path)
                    .map_err(|e| CoreError::io(&path, e))?;
            }
            return Ok(());
        }
    }

    let Some(parent) = path.parent() else {
        unreachable!("openai.yaml always has a parent");
    };
    let parent = parent.to_path_buf();
    let scoped_parent = crate::ports::confine(&rt.scope, fs, &parent)?;
    fs.create_dir_all(&guard, &scoped_parent)
        .map_err(|e| CoreError::io(&parent, e))?;
    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(root)).map_err(|e| {
        CoreError::new(
            ErrorCode::InvalidRequest,
            format!("Failed to serialize {}: {e}", path.display()),
        )
        .at(&path)
    })?;
    let scoped_path = crate::ports::confine(&rt.scope, fs, &path)?;
    fs.write_atomic(&guard, &scoped_path, yaml.as_bytes())
        .map_err(|e| CoreError::io(&path, e))
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
                .opencode_skill_rules
                .is_denied(name)
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
    // skills.sh entry, fall through to the checks below. A missing entry is
    // therefore a caller bug, not a "no ledger" case: silently skipping the
    // symlink and universal-root carve-outs would misclassify the skill.
    let Some(ledger) = cx.scope_ledgers.get(cx.scope) else {
        unreachable!("scan_inner populates a ledger for every scope it classifies")
    };

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
        return if entry.has_manifest_row {
            (LifecycleOwnerKind::Dotagents, Some(owner))
        } else {
            (LifecycleOwnerKind::WildcardDotagents, Some(owner))
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

pub(crate) fn scope_label(scope: &RootScope) -> &'static str {
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
/// wire id, except `OpenCode`, whose slot is the un-hyphenated CLI name
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
pub(crate) fn deployment_id(
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
    DeploymentId::derived(raw)
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
    OwnerId::derived(raw)
}

/// Content fingerprint over a deployment's whole directory tree: sha256 over
/// the sorted `(relative path, bytes)` pairs, each length-framed as `u64 LE
/// len(rel_path) || rel_path bytes || u64 LE file_len || file bytes`, capped
/// at [`MAX_FOLDER_FILES`] files and [`MAX_FOLDER_BYTES`] total bytes. The
/// truncation edge case (a file so large only part of it fits the
/// remaining byte budget) degrades to "read nothing further", since
/// [`ScopeFs::read_capped`] has no partial-read primitive.
///
/// Consumes `files` as already gathered by [`walk_folder_for_facts`]'s single
/// pass, rather than walking the tree again: the fingerprint's own file set
/// is a byproduct of that one walk, not a second `read_dir` per directory.
/// `skill_md` reuses the bytes [`read_skill_md`] already read for the file at
/// `skill_md.path` when that read covered the whole file, so `SKILL.md`
/// itself is never read a second time here.
fn content_fingerprint(
    fs: &dyn ScopeFs,
    files: &[(PathBuf, PathBuf, u64)],
    skill_md: &SkillMdBytes,
) -> Fingerprint {
    let mut files: Vec<_> = files.to_vec();
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
        match skill_md.read_capped(fs, abs_path, remaining.min(*len)) {
            Ok(bytes) => {
                remaining = remaining.saturating_sub(bytes.len() as u64);
                buf.extend_from_slice(&bytes);
            }
            Err(_) => remaining = 0,
        }
    }
    Fingerprint::of_bytes(&buf)
}

/// The already-read bytes of `<skill_dir>/SKILL.md`, so a folder walk that
/// re-encounters that same file (both hash schemes walk the whole folder,
/// `SKILL.md` included) can reuse them instead of reading the file again.
/// Only usable when the caller's read was not truncated and covered exactly
/// the bytes now wanted; [`SkillMdBytes::read_capped`] falls back to a real
/// read otherwise, so a truncated or oversized `SKILL.md` is still handled
/// correctly, just not from cache.
struct SkillMdBytes<'a> {
    path: &'a Path,
    bytes: &'a [u8],
    truncated: bool,
}

impl SkillMdBytes<'_> {
    fn read_capped(&self, fs: &dyn ScopeFs, path: &Path, want: u64) -> std::io::Result<Vec<u8>> {
        if !self.truncated && path == self.path && want == self.bytes.len() as u64 {
            return Ok(self.bytes.to_vec());
        }
        fs.read_capped(path, want)
    }
}

/// The embedded `cl100k_base` vocab is loaded once per process.
static TOKENIZER: OnceLock<Option<CoreBPE>> = OnceLock::new();

fn tokenizer() -> Option<&'static CoreBPE> {
    TOKENIZER
        .get_or_init(|| tiktoken_rs::cl100k_base().ok())
        .as_ref()
}

/// Token count of `text`, `cl100k_base`. `None` tokenizer (the embedded vocab
/// failed to build, which should never happen) yields 0.
fn count_tokens(text: &str, tokenizer: Option<&CoreBPE>) -> u32 {
    tokenizer.map_or(0, |bpe| bpe.encode_with_special_tokens(text).len() as u32)
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
    /// The same regular files as `hashable`, gathered under
    /// [`content_fingerprint`]'s own unreduced [`MAX_FOLDER_BYTES`]/
    /// [`MAX_FOLDER_FILES`] budget rather than `hashable`'s (which has
    /// `skill_md_bytes.len()` deducted up front): the two schemes' file
    /// sets only diverge right at the byte cap, which real skill folders
    /// never approach, so this second budget only matters there.
    fingerprint_files: Vec<(PathBuf, PathBuf, u64)>,
    fingerprint_total_bytes: u64,
    fingerprint_file_count: usize,
}

impl FactsWalk {
    /// Whether the fingerprint side has spent its own, unreduced
    /// `MAX_FOLDER_FILES`/`MAX_FOLDER_BYTES` budget - the walk may stop only
    /// once this and `truncated` (the hash side's budget) are both true,
    /// since the two run independently and the hash side commonly hits its
    /// smaller/reduced budget first.
    fn fingerprint_done(&self) -> bool {
        self.fingerprint_file_count >= MAX_FOLDER_FILES
            || self.fingerprint_total_bytes >= MAX_FOLDER_BYTES
    }
}

/// Walks `dir` once into `walk`, gathering both [`content_hash`] facts
/// (byte/file counts, the newest mtime) and the file list
/// [`content_fingerprint`] hashes, stopping each independently once its own
/// [`MAX_FOLDER_FILES`]/byte budget is reached - the walk itself only ends
/// once both budgets are spent, so an entry past the hash side's (often
/// smaller, reduced) budget still reaches the fingerprint side's own gate.
/// One `read_dir` and one
/// `symlink_metadata` per entry serves both; before this merge each ran its
/// own recursive walk over the same tree. Never follows a symlinked
/// directory; a symlinked file counts toward `hashable`'s `file_count` but
/// is never opened, hashed, sized into `total_bytes`, or added to
/// `fingerprint_files` (`content_fingerprint` has never counted symlinks).
/// Unreadable entries are skipped rather than failing the whole walk. A
/// per-directory-entry [`OpContext::checkpoint`] means a cancellation here
/// must fail the whole walk (and so the caller's digests) rather than return
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
    if walk.truncated && walk.fingerprint_done() {
        return Ok(());
    }
    let Ok(mut entries) = fs.read_dir(dir) else {
        return Ok(());
    };
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    for entry in entries {
        ctx.checkpoint()?;
        if walk.truncated && walk.fingerprint_done() {
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
                if is_file && !walk.truncated {
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
                if !walk.truncated {
                    // Enforce the remaining byte budget before queuing the
                    // file, not after: a single oversized file must never be
                    // added to the hash queue, only counted as the reason
                    // the hash side stopped. It still falls through to the
                    // fingerprint gate below, which runs on its own budget.
                    let remaining = max_bytes.saturating_sub(walk.total_bytes);
                    if meta.len > remaining {
                        walk.truncated = true;
                    } else {
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
                        if walk.file_count as usize >= MAX_FOLDER_FILES
                            || walk.total_bytes >= max_bytes
                        {
                            walk.truncated = true;
                        }
                    }
                }

                // `content_fingerprint`'s own, unreduced budget: mirrors
                // the old standalone `walk_content_files`'s per-entry gate.
                if walk.fingerprint_file_count < MAX_FOLDER_FILES
                    && walk.fingerprint_total_bytes < MAX_FOLDER_BYTES
                {
                    let fp_remaining =
                        MAX_FOLDER_BYTES.saturating_sub(walk.fingerprint_total_bytes);
                    if meta.len > fp_remaining {
                        walk.fingerprint_total_bytes = MAX_FOLDER_BYTES;
                    } else {
                        walk.fingerprint_total_bytes += meta.len;
                        walk.fingerprint_file_count += 1;
                        if let Ok(rel) = path.strip_prefix(root) {
                            walk.fingerprint_files.push((
                                rel.to_path_buf(),
                                path.clone(),
                                meta.len,
                            ));
                        }
                    }
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
    skill_md: &SkillMdBytes,
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
        match skill_md.read_capped(fs, &file.abs_path, remaining.min(file.len)) {
            Ok(bytes) => {
                remaining = remaining.saturating_sub(bytes.len() as u64);
                hasher.update(&bytes);
            }
            Err(_) => remaining = 0,
        }
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(hex, "{byte:02x}").ok();
    }
    Ok(hex)
}

/// Every content fact about a skill folder that [`DeploymentDto`] carries,
/// gathered from one `SKILL.md` read and one folder walk.
#[derive(Debug, Clone)]
struct ContentFacts {
    frontmatter: Option<frontmatter::SkillFrontmatter>,
    frontmatter_fields: BTreeMap<String, String>,
    has_spec: bool,
    folder_bytes: u64,
    file_count: u32,
    skill_md_tokens: u32,
    description_tokens: u32,
    content_hash: String,
    /// Content fingerprint over the whole directory tree - a different,
    /// whole-folder scheme from `content_hash`, kept for parity with the
    /// desktop's `SkillCandidate`. Computed from the same [`FactsWalk`] that
    /// derives `content_hash`, not a second folder walk.
    content_fingerprint: Fingerprint,
    modified_at: Option<DateTime<Utc>>,
    folder_truncated: bool,
}

impl Default for ContentFacts {
    /// Used only for an unresolved link, whose caller always sets its own
    /// `content_fingerprint: None` rather than reading this field - the
    /// empty-bytes fingerprint here is a placeholder, never surfaced.
    fn default() -> Self {
        ContentFacts {
            frontmatter: None,
            frontmatter_fields: BTreeMap::new(),
            has_spec: false,
            folder_bytes: 0,
            file_count: 0,
            skill_md_tokens: 0,
            description_tokens: 0,
            content_hash: String::new(),
            content_fingerprint: Fingerprint::of_bytes(&[]),
            modified_at: None,
            folder_truncated: false,
        }
    }
}

/// Walks `skill_dir` once and derives every [`ContentFacts`] field from that
/// walk plus the already-read `skill_md_bytes`/`parsed` (so the caller's own
/// `SKILL.md` read and parse, already needed for `description`/
/// `spec_violations`, is never repeated here, and the walk's own encounter
/// with `SKILL.md` as a folder entry reuses those same bytes rather than
/// reading the file again). `skill_md_bytes`' own length is deducted from
/// the `content_hash` walk's byte budget first, since that walk re-hashes
/// `SKILL.md` as part of the folder; `content_fingerprint` keeps its own
/// unreduced budget, unchanged from before this merge.
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

    let skill_md_path = skill_dir.join("SKILL.md");
    let skill_md = SkillMdBytes {
        path: &skill_md_path,
        bytes: skill_md_bytes,
        truncated: skill_md_truncated,
    };
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
        content_fingerprint: content_fingerprint(fs, &walk.fingerprint_files, &skill_md),
        content_hash: content_hash_from_walk(walk.hashable, fs, ctx, MAX_FOLDER_BYTES, &skill_md)?,
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
    let skill_md_path = skill_dir.join("SKILL.md");
    let (bytes, truncated) = fs
        .read_prefix(&skill_md_path, SKILL_MD_MAX_BYTES)
        .map_err(|e| CoreError::io(skill_md_path.clone(), e))?;
    let mut walk = FactsWalk::default();
    walk_folder_for_facts(
        fs,
        ctx,
        skill_dir,
        skill_dir,
        MAX_FOLDER_BYTES.saturating_sub(bytes.len() as u64),
        &mut walk,
    )?;
    let skill_md = SkillMdBytes {
        path: &skill_md_path,
        bytes: &bytes,
        truncated,
    };
    content_hash_from_walk(walk.hashable, fs, ctx, MAX_FOLDER_BYTES, &skill_md)
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
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let inventory = scan(rt, ctx, req);
    // Discard `scan`'s own timing immediately: an error below must leave
    // `ctx` with `diagnose`'s own timing or none, never `scan`'s.
    ctx.take_timing();
    let inventory = inventory?;
    let scan_step = crate::timing::step(clock, "scan", step_start);
    ctx.checkpoint()?;
    let step_start = clock.monotonic();
    let issues = derive_issues(rt.ports.fs.as_ref(), &inventory);
    let derive_step = crate::timing::step(clock, "derive_issues", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "diagnose",
        op_start,
        vec![scan_step, derive_step],
    ));
    Ok(Diagnosis { inventory, issues })
}

/// Reuses [`diagnose`] to find every skill with more than one `Canonical` or
/// `Independent` deployment whose `content_hash` differs from another's:
/// two copies of one skill that hold different bytes. Never merges and
/// writes nothing - the caller opens both paths in the user's editor side
/// by side, the same way `git` opens a merge conflict.
///
/// Preconditions: shared lease (through [`diagnose`]'s [`scan`]).
pub fn diagnose_conflict(
    rt: &Runtime,
    ctx: &OpContext,
    _req: &crate::dto::DiagnoseConflictRequest,
) -> Result<crate::dto::ConflictReport, CoreError> {
    let diagnosis = diagnose(rt, ctx, &ScanRequest::default())?;
    Ok(crate::dto::ConflictReport {
        conflicts: conflicts_in(&diagnosis.inventory),
    })
}

/// Every pair of `Canonical`/`Independent` deployments of one skill whose
/// `content_hash` differs, over an inventory already scanned. Shared by
/// [`diagnose_conflict`] and [`fix_skill`] so a fix for one skill does not
/// pay for a second full scan just to find that skill's own conflicts.
fn conflicts_in(inventory: &Inventory) -> Vec<crate::dto::ConflictSummary> {
    let mut conflicts = Vec::new();
    for skill in &inventory.skills {
        let copies: Vec<&DeploymentDto> = skill
            .deployments
            .iter()
            .filter(|d| {
                matches!(
                    d.backing,
                    BackingRelationship::Canonical | BackingRelationship::Independent
                )
            })
            .collect();
        for i in 0..copies.len() {
            for j in (i + 1)..copies.len() {
                let (a, b) = (copies[i], copies[j]);
                if a.content_hash != b.content_hash {
                    conflicts.push(crate::dto::ConflictSummary {
                        skill: skill.name.clone(),
                        message: format!(
                            "{} and {} hold different bytes for `{}`",
                            a.path.display(),
                            b.path.display(),
                            skill.name.0
                        ),
                        path_a: a.path.clone(),
                        path_b: b.path.clone(),
                    });
                }
            }
        }
    }
    conflicts
}

/// Runs the doctor invariants from `docs/action-map/lifecycle-states.md`
/// for one skill and applies whichever repair exists.
///
/// A [`IssueKind::RepairableFrontmatter`] issue is repaired the same way
/// [`preview_frontmatter_repair`]/[`apply_frontmatter_repair`] would (this
/// is the dispatch those two entry points gain, per the migration
/// mapping). Every other issue - a dangling link ([`IssueKind::BrokenLink`],
/// invariant 1, repaired by the desktop's journaled `repair_skill_link`, not
/// duplicated here), a stale registry or lockfile entry (invariants 2 and 3,
/// detect-only - see [`crate::doctor`]'s module doc comment for why), a
/// folder in two states at once (invariant 4), or a quarantine over its
/// retention cap (invariant 5, unit 3.9's follow-up: pruning needs a lease
/// and a journal entry this op does not take) - is returned unfixed, named
/// with its path. Conflicts ([`diagnose_conflict`]'s own logic, reused
/// through [`conflicts_in`] against the same scan rather than a second one)
/// are reported alongside, never written.
///
/// Preconditions: none beyond what the sub-operations this composes need;
/// each runs its own lease.
pub fn fix_skill(
    rt: &Runtime,
    ctx: &OpContext,
    req: &crate::dto::FixSkillRequest,
) -> Result<crate::dto::FixSkillOutcome, CoreError> {
    use crate::dto::UnrepairedIssue;

    ctx.checkpoint()?;
    let diagnosis = diagnose(
        rt,
        ctx,
        &ScanRequest {
            skills: vec![req.skill.clone()],
            timings: false,
        },
    );
    ctx.take_timing();
    let diagnosis = diagnosis?;

    let mut applied = Vec::new();
    let mut unrepaired = Vec::new();

    // `BrokenLink` issues are named by `check_link_resolves_in_root`
    // (invariant 1) below instead of here, so a dangling link is reported
    // once, not once per source.
    for issue in diagnosis
        .issues
        .iter()
        .filter(|issue| issue.skill == req.skill && issue.kind != IssueKind::BrokenLink)
    {
        let NextAction::PreviewRepair { deployment_id } = &issue.next_action else {
            unrepaired.push(UnrepairedIssue {
                path: issue_path(&diagnosis, issue),
                message: issue.message.clone(),
                kind: unrepaired_issue_kind(issue.kind),
            });
            continue;
        };
        let preview = preview_frontmatter_repair(
            rt,
            ctx,
            &RepairPreviewRequest {
                deployment_id: deployment_id.clone(),
            },
        );
        ctx.take_timing();
        let preview = match preview {
            Ok(preview) => preview,
            Err(error) => {
                unrepaired.push(UnrepairedIssue {
                    path: issue_path(&diagnosis, issue),
                    message: error.message,
                    kind: crate::dto::UnrepairedIssueKind::Frontmatter,
                });
                continue;
            }
        };
        let outcome = apply_frontmatter_repair(
            rt,
            ctx,
            &RepairApplyRequest {
                preview: preview.clone(),
                mode: RepairApplyMode::ApplyFix,
            },
        );
        ctx.take_timing();
        match outcome {
            Ok(RepairOutcome::Applied {
                event_id,
                deployment_id,
            }) => applied.push(crate::dto::FixApplied::FrontmatterRepair {
                deployment_id,
                event_id,
            }),
            Ok(RepairOutcome::AlreadyApplied { .. }) => {}
            Err(error) => unrepaired.push(UnrepairedIssue {
                path: preview.path.clone(),
                message: error.message,
                kind: crate::dto::UnrepairedIssueKind::Frontmatter,
            }),
        }
    }

    let fs = rt.ports.fs.as_ref();
    let home = &rt.scope.home.lexical;
    for violation in crate::doctor::check_link_resolves_in_root(&diagnosis)
        .into_iter()
        .chain(crate::doctor::check_registry_entry_has_folder(fs, home))
        .chain(crate::doctor::check_lockfile_entry_has_folder(
            fs,
            home,
            &diagnosis.inventory,
        ))
        .chain(crate::doctor::check_no_folder_in_two_states(
            &diagnosis.inventory,
            std::slice::from_ref(&req.skill),
        ))
        .filter(|violation| violation.skill.as_ref() == Some(&req.skill))
    {
        unrepaired.push(UnrepairedIssue {
            kind: unrepaired_issue_kind_for_invariant(violation.invariant),
            path: violation.path,
            message: violation.message,
        });
    }
    // Global, not skill-scoped: reported whenever a fix for any skill
    // happens to run, the same way the pre-fix code did.
    if let Some(violation) = crate::doctor::check_quarantine_within_cap(fs, home)
        .into_iter()
        .next()
    {
        unrepaired.push(UnrepairedIssue {
            kind: unrepaired_issue_kind_for_invariant(violation.invariant),
            path: violation.path,
            message: violation.message,
        });
    }

    let conflicts = conflicts_in(&diagnosis.inventory);
    ctx.take_timing();

    Ok(crate::dto::FixSkillOutcome {
        skill: req.skill.clone(),
        applied,
        unrepaired,
        conflicts,
    })
}

/// Maps a diagnosed [`IssueKind`] to the coarser [`UnrepairedIssueKind`] a
/// caller branches on. `RepairableFrontmatter` issues never reach here
/// unrepaired at this kind (see the two call sites below that classify
/// their own repair-attempt failures), so any unmatched kind falls back to
/// `Other` rather than claiming a category the caller can't act on.
fn unrepaired_issue_kind(kind: IssueKind) -> crate::dto::UnrepairedIssueKind {
    match kind {
        IssueKind::UnreadableLink => crate::dto::UnrepairedIssueKind::Link,
        IssueKind::SpecViolation | IssueKind::RepairableFrontmatter => {
            crate::dto::UnrepairedIssueKind::Frontmatter
        }
        _ => crate::dto::UnrepairedIssueKind::Other,
    }
}

/// Maps a [`DoctorInvariant`] to the coarser [`UnrepairedIssueKind`] a
/// caller branches on.
fn unrepaired_issue_kind_for_invariant(
    invariant: crate::doctor::DoctorInvariant,
) -> crate::dto::UnrepairedIssueKind {
    match invariant {
        crate::doctor::DoctorInvariant::LinkResolvesInRoot => crate::dto::UnrepairedIssueKind::Link,
        _ => crate::dto::UnrepairedIssueKind::Other,
    }
}

/// Resolves `issue`'s own deployment to its path via `diagnosis`'s
/// inventory, so an unrepaired issue names a real file instead of an empty
/// path. `IssueKind::Duplicate` (from `duplicate_issues`) carries no
/// `deployment_id`, since the issue is about the skill having two
/// deployments rather than one of them; fall back to the skill's first
/// `Canonical` or `Independent` deployment (never `LinkedTo`, which just
/// points back at one of the others) so the path still names a real file.
fn issue_path(diagnosis: &Diagnosis, issue: &Issue) -> PathBuf {
    issue
        .deployment_id
        .as_ref()
        .and_then(|id| {
            diagnosis
                .inventory
                .skills
                .iter()
                .flat_map(|skill| &skill.deployments)
                .find(|deployment| &deployment.id == id)
        })
        .or_else(|| {
            diagnosis
                .inventory
                .skills
                .iter()
                .find(|skill| skill.name == issue.skill)
                .and_then(|skill| {
                    skill.deployments.iter().find(|deployment| {
                        matches!(
                            deployment.backing,
                            BackingRelationship::Canonical | BackingRelationship::Independent
                        )
                    })
                })
        })
        .map(|deployment| deployment.path.clone())
        .unwrap_or_default()
}

/// Runs `scan` and checks every deployment's currency against its install
/// method's source - skills.sh by lock hash against tree hash, dotagents by
/// pinned commit against newest commit, plugin by cache version against the
/// marketplace manifest, manual/in-repo/fork never. One entry per skill
/// name; see [`crate::skill_update_check`] for the rule each method follows.
///
/// Preconditions: same as [`scan`]. Ports for skills.sh, dotagents, and
/// plugin lookups are supplied directly, not through [`crate::ports::Ports`]:
/// unlike a mutation's fs/lease/journal ports, these three are read-only
/// network lookups this one op needs, so a direct parameter avoids adding
/// three more `Option<Arc<dyn _>>` fields (and every existing `Ports`
/// literal in this crate's other tests) for a single caller.
pub fn outdated(
    rt: &Runtime,
    ctx: &OpContext,
    req: &ScanRequest,
    tree_lookup: &dyn crate::skill_update_check::SourceTreeLookup,
    commit_lookup: &dyn crate::skill_update_check::CommitLookup,
    plugin_lookup: &dyn crate::skill_update_check::PluginManifestLookup,
) -> Result<BTreeMap<String, crate::skill_update_check::Currency>, CoreError> {
    let inventory = scan(rt, ctx, req)?;
    let targets: Vec<crate::skill_update_check::OutdatedTarget> = inventory
        .skills
        .iter()
        .filter_map(outdated_target)
        .collect();
    Ok(crate::skill_update_check::outdated(
        rt.ports.fs.as_ref(),
        &rt.scope.home.lexical,
        &targets,
        tree_lookup,
        commit_lookup,
        plugin_lookup,
    ))
}

/// Picks the deployment that decides `skill`'s currency rule, and builds the
/// `OutdatedTarget` for it. A skill deployed by more than one install method
/// is classified by provenance precedence (dotagents beats plugin beats
/// skills-sh beats in-repo beats manual, via `SourceKind`'s derived `Ord`),
/// not by whichever deployment `scan` happened to list first - matching
/// `apps/desktop`'s `provenance::classify_source_kind` precedence.
fn outdated_target(skill: &InstalledSkillDto) -> Option<crate::skill_update_check::OutdatedTarget> {
    let deployment = skill
        .deployments
        .iter()
        .min_by_key(|deployment| deployment.source_kind)?;
    let plugin = deployment
        .plugin
        .as_ref()
        .map(|p| (p.marketplace.clone(), p.plugin.clone(), p.version.clone()));
    Some(crate::skill_update_check::OutdatedTarget {
        name: skill.name.0.clone(),
        source_kind: deployment.source_kind,
        plugin,
    })
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
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let catalog = &rt.ports.catalog;
    if let Some(unknown) = req.harnesses.iter().find(|id| catalog.get(id).is_none()) {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!("no catalog row for harness `{}`", unknown.as_str()),
        ));
    }
    let step_start = clock.monotonic();
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
    let tools_step = crate::timing::step(clock, "tools_lookup", step_start);
    let step_start = clock.monotonic();
    let harnesses = catalog
        .facts
        .iter()
        .filter(|f| req.harnesses.is_empty() || req.harnesses.contains(&f.id))
        .map(|f| {
            let observed = req.observe.then(|| observe_harness(rt, f));
            CapabilityReport::from_facts(f, observed)
        })
        .collect();
    let harnesses_step = crate::timing::step(clock, "harness_reports", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "capabilities",
        op_start,
        vec![tools_step, harnesses_step],
    ));
    Ok(Capabilities { harnesses, tools })
}

/// Detects, per first-class harness, whether it exists on this machine: the
/// executable on `PATH`, its version and install method (probed with
/// `--version`, evidence-backed), whether it is configured, and whether it
/// has run. Pure over ports: this is the only op that touches
/// `rt.ports.spawner`; `scan` and every other op never do.
///
/// Preconditions: none. Without a `ToolLookup` port every executable reads
/// as absent; without a `ProcessSpawner` port version and install method
/// stay `Unknown` even when the binary is found.
pub fn harnesses(
    rt: &Runtime,
    ctx: &OpContext,
    _req: &HarnessesRequest,
) -> Result<HarnessReport, CoreError> {
    ctx.checkpoint()?;
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let ports = DetectionPorts {
        fs: rt.ports.fs.as_ref(),
        home: &rt.scope.home.canonical,
        tools: rt.ports.tools.as_deref(),
        spawner: rt.ports.spawner.as_deref(),
    };
    let harnesses = builtin_adapters()
        .iter()
        .map(|adapter| adapter.detect(&ports))
        .collect();
    let detect_step = crate::timing::step(clock, "detect_harnesses", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "harnesses",
        op_start,
        vec![detect_step],
    ));
    Ok(HarnessReport { harnesses })
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
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let _guard = acquire_shared(rt.ports.leases.as_ref(), &rt.scope)?;
    let step_start = clock.monotonic();
    let inventory = scan_inner(
        rt,
        ctx,
        &ScanRequest {
            skills: Vec::new(),
            timings: false,
        },
    );
    // Discard `scan_inner`'s own timing immediately: an error below must
    // leave `ctx` with this op's own timing or none, never the nested
    // scan's.
    ctx.take_timing();
    let inventory = inventory?;
    let scan_step = crate::timing::step(clock, "scan", step_start);
    let step_start = clock.monotonic();
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
    let propose_step = crate::timing::step(clock, "propose_repair", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "preview_frontmatter_repair",
        op_start,
        vec![scan_step, propose_step],
    ));
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
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
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

    let step_start = clock.monotonic();
    let session = crate::ports::MutationSession::begin(rt, ctx);
    // Discard the nested scan's timing immediately: an error below must
    // leave `ctx` with this op's own timing or none, never the nested
    // scan's.
    ctx.take_timing();
    let mut session = session?;
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
    let begin_step = crate::timing::step(clock, "begin_session", step_start);

    let step_start = clock.monotonic();
    let fs = rt.ports.fs.as_ref();
    let (path, bytes, content) = read_skill_md_text(fs, &deployment.path)?;
    let live_fingerprint = Fingerprint::of_bytes(&bytes);
    if live_fingerprint == preview.proposed_fingerprint {
        let verify_step = crate::timing::step(clock, "read_and_verify", step_start);
        ctx.record_timing(crate::timing::op_timing(
            clock,
            "apply_frontmatter_repair",
            op_start,
            vec![begin_step, verify_step],
        ));
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
    let verify_step = crate::timing::step(clock, "read_and_verify", step_start);

    let step_start = clock.monotonic();
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
    let post_fingerprint = crate::events::fingerprint_path(fs, &path)?.ok_or_else(|| {
        CoreError::new(
            ErrorCode::ExecutionFailed,
            "the file just written is missing on the immediate re-read",
        )
        .at(&path)
    })?;
    session.store.finish(
        &session.guard,
        &id,
        crate::events::EventStatus::Done,
        Some(post_fingerprint),
    )?;

    session.finish(rt, ctx);
    let write_step = crate::timing::step(clock, "write_and_record", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "apply_frontmatter_repair",
        op_start,
        vec![begin_step, verify_step, write_step],
    ));
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
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let Some(store) = rt
        .ports
        .history
        .open(&rt.scope, HistoryAccess::ReadIfExists)?
    else {
        ctx.record_timing(crate::timing::op_timing(
            clock,
            "list_events",
            op_start,
            vec![crate::timing::step(clock, "open_store", step_start)],
        ));
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
    let mut dtos: Vec<EventDto> = rows
        .iter()
        .map(super::events::EventRecord::to_dto)
        .collect();
    let list_step = crate::timing::step(clock, "open_and_list", step_start);
    let step_start = clock.monotonic();
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
            let live = live
                .as_ref()
                .map_or("absent", super::identity::Fingerprint::bare_hex);
            dto.drift = if live == post {
                DriftState::Clean
            } else {
                DriftState::Drifted
            };
        }
    }
    let drift_step = crate::timing::step(clock, "check_drift", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "list_events",
        op_start,
        vec![list_step, drift_step],
    ));
    Ok(dtos)
}

/// What a restore will do to the live file, decided before the claim on
/// `reverted_by` so the claim is only taken once nothing else can fail.
enum RestorePlan {
    /// The original event's backup recorded the path as absent.
    RemoveIfPresent,
    /// The bytes to write back, read from the original event's backup.
    Write(Vec<u8>),
    /// A directory's files to write back, read from the original event's
    /// backup, paths relative to the directory itself. Applied through
    /// [`fsops::stage`]/[`fsops::swap`] (see [`restore_event`]'s mutation
    /// step) rather than [`ScopeFs::write_atomic`], which only ever writes
    /// one file.
    WriteDir(Vec<(PathBuf, Vec<u8>)>),
}

/// [`RestorePlan::WriteDir`]'s mutation step: stages `files` beside `path`
/// under its own journal root (the same lease/journal primitives
/// `ops::update`'s own `Copy` method uses) and swaps the staged folder into
/// `path`, which - since a folder already sits there - quarantines the
/// pre-restore tree the same way an update's own swap quarantines the
/// pre-update tree. That quarantined copy is not itself wired to a further
/// undo; restoring a restore is out of this op's scope.
fn restore_write_dir(
    rt: &Runtime,
    guard: &ExclusiveGuard,
    path: &Path,
    files: &[(PathBuf, Vec<u8>)],
) -> Result<(), CoreError> {
    let universal_root = path.parent().ok_or_else(|| {
        CoreError::new(ErrorCode::Io, "restore target has no parent directory").at(path)
    })?;
    let final_name = path
        .file_name()
        .ok_or_else(|| CoreError::new(ErrorCode::Io, "restore target has no file name").at(path))?;
    let fs = rt.ports.fs.as_ref();
    ops_install::ensure_journal_root(rt, guard, fs)?;
    let journal_root = ops_install::journal_root(&rt.scope.home.lexical);
    let journal = FsJournal::new(journal_root, rt.ports.fs.clone());

    let root = fsops::Root::open(fs, universal_root.to_path_buf())
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    let plan_id = PlanId(rt.ports.ids.next_event_id().0);
    let plan = PlanWriter::begin(
        &journal,
        guard,
        plan_id,
        rt.ports.clock.now(),
        format!("restore {}", path.display()),
        universal_root.to_path_buf(),
        Vec::new(),
    )
    .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?;

    let staged = fsops::stage(&root, &plan, files)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    // Same directory the doctor prune and check sweep, not a
    // restore-specific name - see `ops_update`'s module doc for the same
    // fix applied there.
    let quarantine_dir = Path::new(crate::doctor::QUARANTINE_DIR_NAME);
    fsops::swap(&root, &plan, Path::new(final_name), &staged, quarantine_dir)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(universal_root))?;
    plan.finish(PlanStatus::Done)
        .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()))?;
    Ok(())
}

/// Reverts one event.
///
/// Preconditions: exclusive lease; the claim on `reverted_by` succeeds
/// ([`ErrorCode::AlreadyReverted`] otherwise); the live fingerprint matches
/// the recorded one unless `force` ([`ErrorCode::DriftConflict`] otherwise).
/// The restore is itself an event with its own backup.
/// Restores a Claude Code per-skill link toggle recorded as a
/// [`crate::events::SymlinkInverse`]. Kept apart from the `restore_backup`
/// path below: a symlink toggle has no bytes to diff, only "present at this
/// target" vs "absent", so the restore's own undo comes from
/// [`ScopeFs::read_link`]/[`ScopeFs::symlink_metadata`] rather than a
/// fingerprinted byte backup. Two drift branches, each with a `force`
/// bypass: the Recreate arm refuses when the recorded target no longer
/// exists; the Remove arm refuses when the live link's resolved target
/// differs from the recorded one, or when no target was recorded.
fn restore_symlink_event(
    rt: &Runtime,
    ctx: &OpContext,
    mut session: crate::ports::MutationSession,
    target: &crate::events::EventRecord,
    inverse: &crate::events::SymlinkInverse,
    force: bool,
    // `op_start`/`begin_step` travel together (the whole op's clock start
    // and the timing of the step already run before this call) - bundled so
    // adding `force` above didn't need a `too_many_arguments` allow.
    (op_start, begin_step): (Duration, crate::timing::StepTiming),
) -> Result<RestoreOutcome, CoreError> {
    let clock = rt.ports.clock.as_ref();
    let step_start = clock.monotonic();
    let fs = rt.ports.fs.as_ref();

    let path = match inverse {
        crate::events::SymlinkInverse::Recreate { path, .. }
        | crate::events::SymlinkInverse::Remove { path, .. } => path.clone(),
    };

    let restore_id = rt.ports.ids.next_event_id();
    // The restore's own inverse is the opposite of what the restore is about
    // to do, not a copy of the inverse it is applying: applying `Recreate`
    // puts a link at `path` (so undoing that restore must remove it), and
    // applying `Remove` takes a link away (so undoing that restore must
    // recreate it, at whatever it currently points to).
    let restore_inverse = match inverse {
        crate::events::SymlinkInverse::Recreate { path, target } => {
            crate::events::remove_symlink_inverse(path, Some(target))
        }
        crate::events::SymlinkInverse::Remove { path, target } => match fs.read_link(path).ok() {
            Some(current_target) => crate::events::recreate_symlink_inverse(
                path,
                &crate::fsops::join_lexical(path.parent().unwrap_or(path), &current_target),
            ),
            None => crate::events::remove_symlink_inverse(path, target.as_deref()),
        },
    };
    let draft = crate::events::EventDraft {
        kind: crate::events::EventKind::Restore,
        skill: target.skill.clone(),
        harness: target.harness.clone(),
        scope: target.scope.clone(),
        project_path: target.project_path.clone(),
        payload: serde_json::json!({ "target_event": target.id.0 }),
        inverse: Some(restore_inverse),
        backup_dir: None,
    };
    session.store.record(&session.guard, &restore_id, &draft)?;

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

    let mutation_result: Result<(), CoreError> = match inverse {
        crate::events::SymlinkInverse::Recreate { path, target } => {
            // A dangling link is never useful, so `force` does not bypass
            // this check the way it bypasses the `restore_backup` path's
            // fingerprint drift: there is no live state to force past, only
            // a target that no longer exists.
            if fs.symlink_metadata(target).is_err() {
                Err(CoreError::new(
                    ErrorCode::DriftConflict,
                    format!(
                        "{} no longer exists; undo would create a dangling link",
                        target.display()
                    ),
                )
                .at(target))
            } else {
                let scoped_target = crate::ports::confine(&rt.scope, fs, target)?;
                let scoped_link = crate::ports::confine(&rt.scope, fs, path)?;
                fs.symlink(&session.guard, &scoped_target, &scoped_link)
                    .map_err(|e| CoreError::io(path, e))
            }
        }
        crate::events::SymlinkInverse::Remove { path, target } => match fs.symlink_metadata(path) {
            Ok(meta) if meta.kind == FileKind::Symlink => {
                // Unlike the target check above, a drifted *target* is
                // survivable with `force`: the path is still a link, so
                // removing it only takes back what this event's own undo
                // owns, even if something retargeted it since. A non-symlink
                // at the path is never removed, `force` or not - that would
                // delete bytes this event never wrote. `target`, when
                // present, is always the resolved absolute form (see the
                // recording site in `set_claude_code_switch`), so the live
                // link is resolved the same way before the comparison: a
                // relative link that still resolves to the recorded target
                // must pass, not be flagged as drifted for a spelling
                // difference alone. A row with no recorded target (written
                // before that field existed) has nothing to compare
                // against, so it is force-only rather than ever passing the
                // drift check on its own.
                let drifted = match target {
                    Some(target) => {
                        let live_target = fs.read_link(path).ok().map(|raw| {
                            crate::fsops::join_lexical(path.parent().unwrap_or(path), &raw)
                        });
                        live_target.as_deref() != Some(target.as_path())
                    }
                    None => true,
                };
                if !force && drifted {
                    let reason = match target {
                        Some(target) => format!(
                            "{} no longer points at {}; pass force to remove it anyway",
                            path.display(),
                            target.display()
                        ),
                        None => format!(
                            "{} recorded no target; pass --force to delete the link",
                            path.display()
                        ),
                    };
                    Err(CoreError::new(ErrorCode::DriftConflict, reason).at(path))
                } else {
                    let scoped_link = crate::ports::confine(&rt.scope, fs, path)?;
                    fs.remove_file(&session.guard, &scoped_link)
                        .map_err(|e| CoreError::io(path, e))
                }
            }
            Ok(_) => Err(CoreError::new(
                ErrorCode::DriftConflict,
                format!(
                    "{} is no longer a symlink; refusing to delete it",
                    path.display()
                ),
            )
            .at(path)),
            Err(_) => Ok(()),
        },
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

    session.store.finish(
        &session.guard,
        &restore_id,
        crate::events::EventStatus::Done,
        None,
    )?;

    session.finish(rt, ctx);
    let restore_step = crate::timing::step(clock, "restore_write", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "restore_event",
        op_start,
        vec![begin_step, restore_step],
    ));
    Ok(RestoreOutcome {
        restore_event_id: restore_id,
        reverted_event_id: target.id.clone(),
        restored_paths: vec![path],
    })
}

/// Reverts one event using its recorded inverse.
///
/// Preconditions: exclusive lease; the event must exist, must not already be
/// reverted, and must carry an inverse this build understands
/// ([`crate::dto::RestoreCapability::Yes`]). A [`crate::events::SymlinkInverse`]
/// dispatches to [`restore_symlink_event`]; every other kind uses the
/// `restore_backup` shape below. Backs up the live state under the new
/// restore event's own id before applying the inverse, so the restore is
/// itself restorable and `force` never destroys the only copy of anything.
pub fn restore_event(
    rt: &Runtime,
    ctx: &OpContext,
    req: &RestoreRequest,
) -> Result<RestoreOutcome, CoreError> {
    ctx.checkpoint()?;
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let session = crate::ports::MutationSession::begin(rt, ctx);
    // Discard the nested scan's timing immediately: an error below must
    // leave `ctx` with this op's own timing or none, never the nested
    // scan's.
    ctx.take_timing();
    let mut session = session?;

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
        crate::dto::RestoreCapability::NotCompleted { status } => {
            return Err(CoreError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "event {} did not complete (status: {status}); its inverse never moved what it describes",
                    req.event_id.0
                ),
            ))
        }
        crate::dto::RestoreCapability::Yes => {}
    }
    let inverse = target.inverse.as_ref().ok_or_else(|| {
        CoreError::new(
            ErrorCode::Unsupported,
            "restore_capability() reported Yes but the event has no inverse",
        )
    })?;
    let begin_step = crate::timing::step(clock, "begin_session", step_start);
    if let Some(symlink_inverse) = crate::events::parse_symlink_inverse(inverse) {
        return restore_symlink_event(
            rt,
            ctx,
            session,
            &target,
            &symlink_inverse,
            req.force,
            (op_start, begin_step),
        );
    }
    let (path, pre, post) =
        crate::events::parse_restore_backup_inverse(inverse).ok_or_else(|| {
            CoreError::new(
                ErrorCode::Unsupported,
                "restore of this event kind is not implemented",
            )
        })?;
    let step_start = clock.monotonic();

    let fs = rt.ports.fs.as_ref();
    let expected = post.as_deref().unwrap_or("absent");
    let live_fingerprint = crate::events::fingerprint_path(fs, &path)?;
    let live = live_fingerprint
        .as_ref()
        .map_or("absent", super::identity::Fingerprint::bare_hex);
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
            // Read from the backup entry itself (see `BackupEntry::is_dir`'s
            // own doc), not the live path's current type: after a `remove`
            // the live path is absent, which would otherwise always look
            // like "not a directory" and send a directory's restore through
            // the single-file `Write` branch below.
            if entry.is_dir {
                let files = session
                    .store
                    .read_backup_files(backup_dir, &entry.relative)?;
                RestorePlan::WriteDir(files)
            } else {
                let bytes = session
                    .store
                    .read_backup_bytes(backup_dir, &entry.relative)?;
                RestorePlan::Write(bytes)
            }
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
        RestorePlan::WriteDir(files) => restore_write_dir(rt, &session.guard, &path, files),
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
    let restore_step = crate::timing::step(clock, "restore_write", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "restore_event",
        op_start,
        vec![begin_step, restore_step],
    ));
    Ok(RestoreOutcome {
        restore_event_id: restore_id,
        reverted_event_id: target.id,
        restored_paths: vec![path],
    })
}

/// Finds the skill entry that owns `deployment_id` in `inventory`.
pub(crate) fn resolve_skill<'a>(
    inventory: &'a Inventory,
    deployment_id: &DeploymentId,
) -> Result<&'a InstalledSkillDto, CoreError> {
    inventory
        .skills
        .iter()
        .find(|s| s.deployments.iter().any(|d| &d.id == deployment_id))
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::AmbiguousTarget,
                format!("no deployment matches {}", deployment_id.as_str()),
            )
        })
}

/// Finds the Claude Code per-skill link deployment pointing at
/// `target_path`, among `skill`'s other deployments.
///
/// `target_path` is canonicalized here rather than compared lexically: scan
/// records a link's target already canonical (`link_target`), but a
/// deployment's own `path` is lexical, so the two only compare equal once
/// both sides go through the same filesystem, and not, for example, when
/// `target_path`'s ancestry crosses a symlink the test host (or the user's
/// `$HOME`) happens to have, like macOS's `/tmp` -> `/private/tmp`.
pub(crate) fn find_claude_link<'a>(
    skill: &'a InstalledSkillDto,
    target_path: &Path,
    fs: &dyn ScopeFs,
) -> Option<&'a DeploymentDto> {
    find_all_links(skill, target_path, fs)
        .into_iter()
        .find(|d| d.harness.as_ref().map(AgentId::as_str) == Some(AgentId::CLAUDE_CODE))
}

/// Finds every per-harness link deployment pointing at `target_path`, among
/// `skill`'s other deployments - the same canonicalized comparison
/// [`find_claude_link`] uses, generalized to every harness rather than just
/// Claude Code, for `ops::remove`'s own link cleanup (every harness a skill
/// was ever linked into must lose that link, not just Claude Code's).
pub(crate) fn find_all_links<'a>(
    skill: &'a InstalledSkillDto,
    target_path: &Path,
    fs: &dyn ScopeFs,
) -> Vec<&'a DeploymentDto> {
    let Ok(canonical_target) = fs.canonicalize(target_path) else {
        return Vec::new();
    };
    skill
        .deployments
        .iter()
        .filter(|d| {
            d.backing == BackingRelationship::LinkedTo
                && d.link_target.as_deref() == Some(canonical_target.as_path())
        })
        .collect()
}

pub use crate::ops_install::{install, install_preferences};
pub use crate::ops_remove::remove;
pub use crate::ops_update::{update, update_all};

/// Moves a universal deployment's directory into the parked root.
///
/// Preconditions: exclusive lease; the deployment must resolve exactly once,
/// live at the universal root ([`RootKind::Universal`]), and hold its own
/// bytes ([`BackingRelationship::Canonical`]). Undo is not implemented by
/// this build: the event's `inverse` is `None`, and `restore_event` refuses
/// it ([`ErrorCode::Unsupported`]); use `unpark` instead.
///
/// Sequence, matching `docs/action-map/primitives-and-call-stack.md`'s Park
/// row: the journal row is recorded before any filesystem step, the Claude
/// Code link (if any) is removed first, then the directory is renamed into
/// `.agents/skills-parked`.
pub fn park(rt: &Runtime, ctx: &OpContext, req: &ParkRequest) -> Result<ParkOutcome, CoreError> {
    ctx.checkpoint()?;
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let session = crate::ports::MutationSession::begin(rt, ctx);
    ctx.take_timing();
    let mut session = session?;

    let deployment = session.resolve_exact(&req.deployment_id)?.clone();
    if deployment.root.kind != RootKind::Universal {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "only a universal deployment can be parked",
        )
        .at(&deployment.path));
    }
    if deployment.backing != BackingRelationship::Canonical {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "only the deployment holding the bytes can be parked, not a link",
        )
        .at(&deployment.path));
    }
    let skill = resolve_skill(&session.fresh, &deployment.id)?.clone();
    let fs = rt.ports.fs.as_ref();
    let claude_link = find_claude_link(&skill, &deployment.path, fs).cloned();
    let begin_step = crate::timing::step(clock, "begin_session", step_start);

    let step_start = clock.monotonic();
    let parked_dir = rt
        .scope
        .home
        .lexical
        .join(PARKED_ROOT_RELATIVE)
        .join(&skill.name.0);
    if fs.symlink_metadata(&parked_dir).is_ok() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "a parked deployment already exists for this skill",
        )
        .at(&parked_dir));
    }
    let scope_label = scope_label(&deployment.root.scope).to_string();
    let project_path = match &deployment.root.scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.clone()),
    };

    let id = rt.ports.ids.next_event_id();
    let draft = crate::events::EventDraft {
        kind: crate::events::EventKind::Park,
        skill: skill.name.clone(),
        harness: None,
        scope: Some(scope_label),
        project_path,
        payload: serde_json::json!({
            "deployment_id": deployment.id.as_str(),
            "from": deployment.path,
            "to": parked_dir,
            "claude_link": claude_link.as_ref().map(|l| &l.path),
        }),
        // Undo of a directory move is out of this build's scope: `Park`
        // deliberately carries no inverse.
        inverse: None,
        backup_dir: None,
    };
    session.store.record(&session.guard, &id, &draft)?;

    if let Some(link) = &claude_link {
        let scoped_link = crate::ports::confine(&rt.scope, fs, &link.path)?;
        fs.remove_file(&session.guard, &scoped_link)
            .map_err(|e| CoreError::io(&link.path, e))?;
    }
    let parent = parked_dir.parent().unwrap_or(&parked_dir).to_path_buf();
    let scoped_parent = crate::ports::confine(&rt.scope, fs, &parent)?;
    fs.create_dir_all(&session.guard, &scoped_parent)
        .map_err(|e| CoreError::io(&parent, e))?;
    let scoped_from = crate::ports::confine(&rt.scope, fs, &deployment.path)?;
    let scoped_to = crate::ports::confine(&rt.scope, fs, &parked_dir)?;
    fs.rename(&session.guard, &scoped_from, &scoped_to)
        .map_err(|e| CoreError::io(&deployment.path, e))?;
    // Codex reads the universal root directly rather than through a link,
    // so a `[[skills.config]]` row disabling this skill names the moved
    // path itself; without this, park would leave that row pointing at a
    // directory that no longer exists (docs/action-map/harnesses/codex.md).
    codex_rewrite_skill_path(
        rt,
        ctx,
        &session.guard,
        &deployment.path.join("SKILL.md"),
        &parked_dir.join("SKILL.md"),
    )?;

    session
        .store
        .finish(&session.guard, &id, crate::events::EventStatus::Done, None)?;
    session.finish(rt, ctx);
    let write_step = crate::timing::step(clock, "remove_link_and_rename", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "park",
        op_start,
        vec![begin_step, write_step],
    ));
    Ok(ParkOutcome {
        event_id: id,
        deployment_id: deployment.id,
        parked_path: parked_dir,
    })
}

/// Moves a parked deployment's directory back to the universal root and
/// recreates the Claude Code link it had, if any.
///
/// Preconditions: exclusive lease; the deployment must resolve exactly once,
/// live at the parked root ([`RootKind::Parked`]); nothing may already
/// occupy the universal path this skill would return to.
///
/// This reverses the most recent unreverted `park` event recorded for the
/// skill (matched by `payload.to` naming this deployment's path), per
/// `primitives-and-call-stack.md`'s "the reverse, from the journal entry".
/// A parked directory with no matching `park` row (never parked by this
/// build, or the row aged out) still unparks: the Claude Code link is then
/// simply not recreated.
pub fn unpark(
    rt: &Runtime,
    ctx: &OpContext,
    req: &UnparkRequest,
) -> Result<UnparkOutcome, CoreError> {
    ctx.checkpoint()?;
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let session = crate::ports::MutationSession::begin(rt, ctx);
    ctx.take_timing();
    let mut session = session?;

    let deployment = session.resolve_exact(&req.deployment_id)?.clone();
    if deployment.root.kind != RootKind::Parked {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "only a parked deployment can be unparked",
        )
        .at(&deployment.path));
    }
    let skill = resolve_skill(&session.fresh, &deployment.id)?.clone();

    let park_row = session
        .store
        .list(&crate::events::EventFilter {
            skill: Some(skill.name.clone()),
            limit: DEFAULT_EVENT_LIMIT,
            after: None,
        })?
        .into_iter()
        .find(|row| {
            row.kind == crate::events::EventKind::Park.as_str()
                && row.reverted_by.is_none()
                && row
                    .payload
                    .get("to")
                    .and_then(|v| v.as_str())
                    .map(Path::new)
                    == Some(deployment.path.as_path())
        });
    let claude_link_path = park_row
        .as_ref()
        .and_then(|row| row.payload.get("claude_link"))
        .and_then(|v| v.as_str())
        .map(PathBuf::from);
    let begin_step = crate::timing::step(clock, "begin_session", step_start);

    let step_start = clock.monotonic();
    let restored_dir = rt
        .scope
        .home
        .lexical
        .join(UNIVERSAL_ROOT_RELATIVE)
        .join(&skill.name.0);
    let fs = rt.ports.fs.as_ref();
    if fs.symlink_metadata(&restored_dir).is_ok() {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            "a universal deployment already exists for this skill",
        )
        .at(&restored_dir));
    }
    let scope_label = scope_label(&deployment.root.scope).to_string();
    let project_path = match &deployment.root.scope {
        RootScope::Global => None,
        RootScope::Project(project) => Some(project.0.clone()),
    };

    let id = rt.ports.ids.next_event_id();
    let draft = crate::events::EventDraft {
        kind: crate::events::EventKind::Unpark,
        skill: skill.name.clone(),
        harness: None,
        scope: Some(scope_label),
        project_path,
        payload: serde_json::json!({
            "deployment_id": deployment.id.as_str(),
            "from": deployment.path,
            "to": restored_dir,
            "claude_link": claude_link_path,
        }),
        inverse: None,
        backup_dir: None,
    };
    session.store.record(&session.guard, &id, &draft)?;

    let parent = restored_dir.parent().unwrap_or(&restored_dir).to_path_buf();
    let scoped_parent = crate::ports::confine(&rt.scope, fs, &parent)?;
    fs.create_dir_all(&session.guard, &scoped_parent)
        .map_err(|e| CoreError::io(&parent, e))?;
    let scoped_from = crate::ports::confine(&rt.scope, fs, &deployment.path)?;
    let scoped_to = crate::ports::confine(&rt.scope, fs, &restored_dir)?;
    fs.rename(&session.guard, &scoped_from, &scoped_to)
        .map_err(|e| CoreError::io(&deployment.path, e))?;
    if let Some(link_path) = &claude_link_path {
        let scoped_target = crate::ports::confine(&rt.scope, fs, &restored_dir)?;
        let scoped_link = crate::ports::confine(&rt.scope, fs, link_path)?;
        fs.symlink(&session.guard, &scoped_target, &scoped_link)
            .map_err(|e| CoreError::io(link_path, e))?;
    }
    // Symmetric with `park`'s codex_rewrite_skill_path call: a `park` may
    // have rewritten a `[[skills.config]]` row to the parked `SKILL.md`
    // path, so unpark rewrites it back to the live path.
    codex_rewrite_skill_path(
        rt,
        ctx,
        &session.guard,
        &deployment.path.join("SKILL.md"),
        &restored_dir.join("SKILL.md"),
    )?;

    session
        .store
        .finish(&session.guard, &id, crate::events::EventStatus::Done, None)?;
    session.finish(rt, ctx);
    let write_step = crate::timing::step(clock, "rename_and_relink", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "unpark",
        op_start,
        vec![begin_step, write_step],
    ));
    Ok(UnparkOutcome {
        event_id: id,
        deployment_id: deployment.id,
        restored_path: restored_dir,
    })
}

/// Turns a skill's native per-harness switch on or off.
///
/// Preconditions: exclusive lease; the skill must resolve to exactly one
/// entry in a fresh inventory (an ambiguous name - two entries sharing
/// `req.skill` - is refused before any write, since `OpenCode`'s switch is
/// keyed by name alone and a Codex/Claude Code write under an ambiguous name
/// would silently pick one).
///
/// Journals before the first write, matching `apply_frontmatter_repair`'s
/// shape (`docs/action-map/enable-and-links.md`'s desired state for this
/// command). Codex is the one harness whose switch can touch more than one
/// path - one `[[skills.config]]` row per canonical `SKILL.md` this skill
/// owns - so it is the one case where `toggled` can be less than `total`: a
/// write failure partway through the loop is reported as "N of M", not
/// swallowed, and the paths already toggled are left toggled rather than
/// rolled back (this build's compensating step is the accurate error
/// message, not a cross-path transaction; see the module doc on
/// `harness_switch.rs` for the scope this narrows).
pub fn set_harness_enabled(
    rt: &Runtime,
    ctx: &OpContext,
    req: &SetHarnessEnabledRequest,
) -> Result<SetHarnessEnabledOutcome, CoreError> {
    ctx.checkpoint()?;
    let clock = rt.ports.clock.as_ref();
    let op_start = clock.monotonic();
    let step_start = clock.monotonic();
    let session = crate::ports::MutationSession::begin(rt, ctx);
    ctx.take_timing();
    let mut session = session?;

    let matches: Vec<&InstalledSkillDto> = session
        .fresh
        .skills
        .iter()
        .filter(|s| s.name == req.skill)
        .collect();
    let skill = match matches.as_slice() {
        [one] => (*one).clone(),
        [] => {
            return Err(CoreError::new(ErrorCode::InvalidRequest, "skill not found")
                .at(Path::new(&req.skill.0)))
        }
        _ => {
            return Err(CoreError::new(
                ErrorCode::AmbiguousTarget,
                format!("{} names more than one installed skill", req.skill),
            ))
        }
    };
    let begin_step = crate::timing::step(clock, "begin_session", step_start);

    let step_start = clock.monotonic();
    let fs = rt.ports.fs.as_ref();
    let home = rt.scope.home.lexical.clone();
    let id = rt.ports.ids.next_event_id();
    let kind = if req.enabled {
        crate::events::EventKind::HarnessEnable
    } else {
        crate::events::EventKind::HarnessDisable
    };

    let (toggled, total) = match req.harness.as_str() {
        AgentId::CLAUDE_CODE => set_claude_code_switch(
            rt,
            &mut session,
            fs,
            &home,
            req.project_path.as_deref(),
            &skill,
            &id,
            kind,
            req.enabled,
        )?,
        AgentId::CODEX => set_codex_switch(
            rt,
            &mut session,
            fs,
            req.project_path.as_deref(),
            &skill,
            (&id, kind),
            req.enabled,
        )?,
        AgentId::OPEN_CODE => {
            set_opencode_switch(rt, &mut session, fs, &home, &skill, &id, kind, req.enabled)?
        }
        AgentId::PI => set_pi_switch(rt, &mut session, fs, &home, &skill, &id, kind, req.enabled)?,
        other => {
            return Err(CoreError::new(
                ErrorCode::Unsupported,
                format!("{other} has no native per-skill switch"),
            ))
        }
    };

    session.finish(rt, ctx);
    let write_step = crate::timing::step(clock, "toggle_switch", step_start);
    ctx.record_timing(crate::timing::op_timing(
        clock,
        "set_harness_enabled",
        op_start,
        vec![begin_step, write_step],
    ));
    Ok(SetHarnessEnabledOutcome {
        event_id: id,
        skill: skill.name,
        harness: req.harness.clone(),
        toggled,
        total,
    })
}

/// Creates `dir` and every missing ancestor, one level at a time.
/// [`crate::ports::confine`] canonicalizes a path's parent to prove it lies
/// inside the scope, so it needs that parent to already exist; a home with
/// no `.config` at all makes a single `confine(".config/opencode")` fail
/// before `create_dir_all` ever runs. Walking up to the first existing
/// ancestor and confining one level at a time avoids that.
pub(crate) fn ensure_dir_all(
    rt: &Runtime,
    session: &crate::ports::MutationSession,
    fs: &dyn ScopeFs,
    dir: &Path,
) -> Result<(), CoreError> {
    let mut missing = Vec::new();
    let mut current = dir.to_path_buf();
    while fs.symlink_metadata(&current).is_err() {
        missing.push(current.clone());
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => break,
        }
    }
    for path in missing.into_iter().rev() {
        let scoped = crate::ports::confine(&rt.scope, fs, &path)?;
        fs.create_dir_all(&session.guard, &scoped)
            .map_err(|e| CoreError::io(&path, e))?;
    }
    Ok(())
}

/// Removes or recreates Claude Code's per-skill link under
/// `<project>/.claude/skills/<name>` for a project-scoped row, or
/// `<home>/.claude/skills/<name>` for a global one. One step, so
/// `toggled`/`total` are always `1`/`1` on success. Idempotent: if the link
/// is already in the requested state, the journal row still records a usable
/// inverse but no filesystem call runs.
#[allow(clippy::too_many_arguments)]
fn set_claude_code_switch(
    rt: &Runtime,
    session: &mut crate::ports::MutationSession,
    fs: &dyn ScopeFs,
    home: &Path,
    project_path: Option<&Path>,
    skill: &InstalledSkillDto,
    id: &EventId,
    kind: crate::events::EventKind,
    enabled: bool,
) -> Result<(u32, u32), CoreError> {
    let target_scope = match project_path {
        Some(project) => RootScope::Project(ProjectRef(project.to_path_buf())),
        None => RootScope::Global,
    };
    let canonical_dir = skill
        .deployments
        .iter()
        .find(|d| {
            d.root.kind == RootKind::Universal
                && d.backing == BackingRelationship::Canonical
                && d.root.scope == target_scope
        })
        .map(|d| d.path.clone())
        .ok_or_else(|| {
            CoreError::new(
                ErrorCode::Unsupported,
                "no universal deployment to link Claude Code to",
            )
        })?;
    let claude_skills_dir = match project_path {
        Some(project) => project.join(".claude/skills"),
        None => home.join(".claude/skills"),
    };
    let link_path = claude_skills_dir.join(&skill.name.0);

    // `~/.claude/skills` itself can be a whole-directory symlink into the
    // shared root; that covers every skill at once and has no per-skill slot
    // to toggle. And the per-skill slot can be a real directory (a plain
    // copy) rather than a link. `symlink_metadata` succeeds on both, so only
    // `FileKind::Symlink` counts as "already linked" - anything else at that
    // path is refused rather than torn down by `remove_file`.
    if fs
        .symlink_metadata(&claude_skills_dir)
        .is_ok_and(|f| f.kind == FileKind::Symlink)
    {
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!(
                "{} is a whole-directory link; Claude Code reads every skill through it, so \"{}\" has no per-skill switch",
                claude_skills_dir.display(),
                skill.name
            ),
        )
        .at(&claude_skills_dir));
    }
    let link_kind = fs.symlink_metadata(&link_path).ok().map(|f| f.kind);
    if let Some(kind @ (FileKind::Dir | FileKind::File | FileKind::Other)) = link_kind {
        let kind_name = match kind {
            FileKind::Dir => "a real directory",
            FileKind::File => "a plain file",
            _ => "not a symlink",
        };
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!(
                "{} is {kind_name}, not a per-skill link; removing it would delete Claude Code's copy",
                link_path.display()
            ),
        )
        .at(&link_path));
    }
    let already_linked = link_kind == Some(FileKind::Symlink);

    // A no-op toggle (enable when already linked, disable when already
    // absent) touches no bytes, so it must carry no inverse: an inverse here
    // would undo a mutation that never happened, deleting a link the
    // *previous* state left in place or recreating one that was never
    // removed.
    let no_op = enabled == already_linked;
    let inverse = if no_op {
        None
    } else if enabled {
        Some(crate::events::remove_symlink_inverse(
            &link_path,
            Some(&canonical_dir),
        ))
    } else {
        // Recreates whatever `link_path` actually pointed at, not the
        // current canonical deployment: a link retargeted by hand (or left
        // over from a moved skill) must undo back to its own real target,
        // not silently point the undo at wherever the universal directory
        // happens to be now. `read_link` returns the raw on-disk text,
        // which a Claude Code link written as `../../.agents/skills/<name>`
        // (the shape the scanner resolves and the desktop relinker writes)
        // leaves relative; `confine` refuses anything not absolute, so it
        // is resolved against the link's own parent and lexically
        // collapsed the same way the scanner resolves a link target,
        // before `confine` refuses a target outside the runtime's scope
        // the same way every other cross-boundary link does. The recorded
        // inverse always carries this resolved absolute form, not the raw
        // relative text: `ScopeFs::symlink` only accepts a `ScopedPath`,
        // which `confine` only produces from an absolute path, so there is
        // no port through which undo could recreate the original relative
        // spelling even if it wanted to.
        let raw_target = fs
            .read_link(&link_path)
            .map_err(|e| CoreError::io(&link_path, e))?;
        let real_target =
            crate::fsops::join_lexical(link_path.parent().unwrap_or(&link_path), &raw_target);
        crate::ports::confine(&rt.scope, fs, &real_target)?;
        Some(crate::events::recreate_symlink_inverse(
            &link_path,
            &real_target,
        ))
    };
    let draft = crate::events::EventDraft {
        kind,
        skill: skill.name.clone(),
        harness: Some(AgentId::from(AgentId::CLAUDE_CODE)),
        scope: Some(
            if project_path.is_some() {
                "project"
            } else {
                "global"
            }
            .to_string(),
        ),
        project_path: project_path.map(Path::to_path_buf),
        payload: serde_json::json!({ "skill": skill.name.0, "harness": AgentId::CLAUDE_CODE }),
        inverse,
        backup_dir: None,
    };
    session.store.record(&session.guard, id, &draft)?;

    // Every fallible step after `record` runs inside this closure so a
    // failure anywhere in it - not just the final `symlink`/`remove_file`
    // call - reaches the `finish(Failed)` below. `?` on a step before this
    // closure existed (e.g. `ensure_dir_all`, `confine`) would return
    // straight out of the function and leave the row `pending` forever,
    // which `recover_interrupted` would later treat as an interrupted crash
    // rather than a plain, retryable failure.
    let mutate: Result<(), CoreError> = (|| {
        if enabled && !already_linked {
            ensure_dir_all(rt, session, fs, &claude_skills_dir)?;
            let scoped_target = crate::ports::confine(&rt.scope, fs, &canonical_dir)?;
            let scoped_link = crate::ports::confine(&rt.scope, fs, &link_path)?;
            fs.symlink(&session.guard, &scoped_target, &scoped_link)
                .map_err(|e| CoreError::io(&link_path, e))?;
        } else if !enabled && already_linked {
            let scoped_link = crate::ports::confine(&rt.scope, fs, &link_path)?;
            fs.remove_file(&session.guard, &scoped_link)
                .map_err(|e| CoreError::io(&link_path, e))?;
        }
        Ok(())
    })();
    if let Err(e) = mutate {
        let _ = session
            .store
            .finish(&session.guard, id, crate::events::EventStatus::Failed, None);
        return Err(e);
    }
    session
        .store
        .finish(&session.guard, id, crate::events::EventStatus::Done, None)?;
    Ok((1, 1))
}

/// True when `kind` is a root Codex reads: the shared universal root, or its
/// own harness root, at either scope. Mirrors [`is_opencode_visible_root`]
/// (Codex has no legacy root of its own to add).
fn is_codex_visible_root(kind: &RootKind) -> bool {
    match kind {
        RootKind::Universal => true,
        RootKind::Harness(id) => id.as_str() == AgentId::CODEX,
        RootKind::Legacy(_) | RootKind::Parked | RootKind::PluginCache(_) => false,
    }
}

/// Every canonical `SKILL.md` path Codex sees for `skill`, sorted for a
/// deterministic write order.
fn codex_skill_md_paths(skill: &InstalledSkillDto) -> Vec<PathBuf> {
    // `LinkedTo` deployments point at another deployment's bytes and have no
    // `SKILL.md` of their own to toggle; `Canonical` and `Independent` both
    // hold real bytes on disk, so both need their own row. `scan` groups
    // every harness's copy of a skill under one `InstalledSkillDto`, so
    // without the `is_codex_visible_root` filter this also picked up
    // deployments at roots Codex never reads - a Claude Code copy, a parked
    // root, and so on - writing a `[[skills.config]]` row for a path Codex
    // never resolves, one a later enable would remove as if it were Codex's
    // own.
    let mut paths: Vec<PathBuf> = skill
        .deployments
        .iter()
        .filter(|d| {
            d.backing != BackingRelationship::LinkedTo && is_codex_visible_root(&d.root.kind)
        })
        .map(|d| d.path.join("SKILL.md"))
        .collect();
    paths.sort();
    paths.dedup();
    paths
}

/// Adds or removes one `[[skills.config]]` row per Codex-visible `SKILL.md`
/// path in `~/.codex/config.toml`. A crash partway through leaves the rows
/// already written toggled and reports "N of M" rather than failing silent
/// (`docs/action-map/enable-and-links.md`'s desired state).
fn set_codex_switch(
    rt: &Runtime,
    session: &mut crate::ports::MutationSession,
    fs: &dyn ScopeFs,
    project_path: Option<&Path>,
    skill: &InstalledSkillDto,
    // `id`/`kind` travel together (the journal row's identity and what kind
    // of row it is) - bundled so adding `project_path` above didn't need a
    // `too_many_arguments` allow.
    (id, kind): (&EventId, crate::events::EventKind),
    enabled: bool,
) -> Result<(u32, u32), CoreError> {
    let paths = codex_skill_md_paths(skill);
    let total = u32::try_from(paths.len()).unwrap_or(u32::MAX);
    if paths.is_empty() {
        return Err(CoreError::new(
            ErrorCode::Unsupported,
            "no Codex-visible SKILL.md paths for this skill",
        ));
    }
    let config_path = codex_config_path(&rt.scope.codex_home);
    let scope = Some(
        if project_path.is_some() {
            "project"
        } else {
            "global"
        }
        .to_string(),
    );
    let project_path_buf = project_path.map(Path::to_path_buf);
    let payload = serde_json::json!({
        "skill": skill.name.0,
        "harness": AgentId::CODEX,
        "total": total,
    });

    // A full no-op (every path already in the state this toggle would put
    // it in) must carry no inverse, the same way `set_claude_code_switch`'s
    // no-op branch does: recording a `restore_backup` inverse over bytes
    // this call never wrote would let `undo` "revert" a mutation that never
    // happened, consuming the claim on the real previous change underneath
    // it instead of reaching that.
    let already_matches = {
        let doc = read_codex_config_document(fs, &rt.scope.codex_home)?;
        paths
            .iter()
            .all(|path| codex_find_row_index(&doc, path).is_some() != enabled)
    };
    if already_matches {
        let draft = crate::events::EventDraft {
            kind,
            skill: skill.name.clone(),
            harness: Some(AgentId::from(AgentId::CODEX)),
            scope,
            project_path: project_path_buf,
            payload,
            inverse: None,
            backup_dir: None,
        };
        session.store.record(&session.guard, id, &draft)?;
        session
            .store
            .finish(&session.guard, id, crate::events::EventStatus::Done, None)?;
        return Ok((total, total));
    }

    let manifest =
        session
            .store
            .backup_paths(&session.guard, id, std::slice::from_ref(&config_path))?;
    let pre_fingerprint = manifest.entries.first().and_then(|e| e.fingerprint.clone());
    let inverse =
        crate::events::restore_backup_inverse(&config_path, pre_fingerprint.as_ref(), None);
    let draft = crate::events::EventDraft {
        kind,
        skill: skill.name.clone(),
        harness: Some(AgentId::from(AgentId::CODEX)),
        scope,
        project_path: project_path_buf,
        payload,
        inverse: Some(inverse),
        backup_dir: Some(manifest.backup_dir.clone()),
    };
    session.store.record(&session.guard, id, &draft)?;

    // See `set_claude_code_switch`'s matching comment: everything after
    // `record` that can fail - `ensure_dir_all`, `confine`, each loop
    // iteration's `read_capped`/`write_atomic`, the final
    // `fingerprint_path` - runs inside this closure so every error path
    // reaches `finish(Failed)` below, not just the write that used to be the
    // last statement in this function.
    let mutate: Result<(u32, Option<Fingerprint>), CoreError> = (|| {
        let config_parent = config_path.parent().unwrap_or(&config_path).to_path_buf();
        ensure_dir_all(rt, session, fs, &config_parent)?;
        let scoped_config = crate::ports::confine(&rt.scope, fs, &config_path)?;
        let mut toggled: u32 = 0;
        for path in &paths {
            let existing =
                match fs.read_capped(
                    &config_path,
                    crate::harness_switch::HARNESS_CONFIG_MAX_BYTES,
                ) {
                    Ok(bytes) => Some(String::from_utf8(bytes).map_err(|e| {
                        CoreError::new(ErrorCode::Io, e.to_string()).at(&config_path)
                    })?),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(CoreError::io(&config_path, e)),
                };
            let mut doc: toml_edit::DocumentMut =
                existing.unwrap_or_default().parse().map_err(|e| {
                    CoreError::new(ErrorCode::Io, format!("config.toml is not valid TOML: {e}"))
                        .at(&config_path)
                })?;
            codex_write_disabled_row(&mut doc, path, !enabled).map_err(|e| e.at(&config_path))?;
            let new_text = doc.to_string();
            fs.write_atomic(&session.guard, &scoped_config, new_text.as_bytes())
                .map_err(|e| {
                    CoreError::new(
                        ErrorCode::Incomplete,
                        format!(
                            "{toggled} of {total} Codex paths toggled for {}: {e}",
                            skill.name
                        ),
                    )
                    .at(&config_path)
                })?;
            toggled += 1;
        }
        let post_fingerprint = crate::events::fingerprint_path(fs, &config_path)?;
        Ok((toggled, post_fingerprint))
    })();

    match mutate {
        Ok((toggled, post_fingerprint)) => {
            session.store.finish(
                &session.guard,
                id,
                crate::events::EventStatus::Done,
                post_fingerprint,
            )?;
            Ok((toggled, total))
        }
        Err(e) => {
            let _ =
                session
                    .store
                    .finish(&session.guard, id, crate::events::EventStatus::Failed, None);
            Err(e)
        }
    }
}

/// True when `kind` is a root `OpenCode` reads: the shared universal root, or
/// its own harness/legacy root.
fn is_opencode_visible_root(kind: &RootKind) -> bool {
    match kind {
        RootKind::Universal => true,
        RootKind::Harness(id) | RootKind::Legacy(id) => id.as_str() == AgentId::OPEN_CODE,
        RootKind::Parked | RootKind::PluginCache(_) => false,
    }
}

/// Refuses an `OpenCode` toggle when `skill.name` resolves to more than one
/// OpenCode-visible location - global plus a project, or two different
/// projects. `set_opencode_switch` writes one name-keyed
/// `permission.skill.<name>` entry in the global `opencode.json`; scan folds
/// same-named deployments across scopes into this one `InstalledSkillDto`,
/// so without this guard a toggle aimed at one project's copy would also
/// silently deny (or allow) an unrelated global copy sharing the name. Named
/// after the desktop's pre-core `refuse_opencode_name_collision`
/// (`apps/desktop/src-tauri/src/skills/skill_harness_disable.rs`), ported
/// here since scan no longer gives the caller distinct deployments to check
/// against.
fn refuse_opencode_name_collision(skill: &InstalledSkillDto) -> Result<(), CoreError> {
    let mut by_scope: std::collections::BTreeMap<String, PathBuf> =
        std::collections::BTreeMap::new();
    for deployment in &skill.deployments {
        if !is_opencode_visible_root(&deployment.root.kind) {
            continue;
        }
        let key = match &deployment.root.scope {
            RootScope::Global => "global".to_string(),
            RootScope::Project(project) => format!("project:{}", project.0.display()),
        };
        by_scope
            .entry(key)
            .or_insert_with(|| deployment.path.clone());
    }
    if by_scope.len() > 1 {
        let paths = by_scope
            .values()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(CoreError::new(
            ErrorCode::InvalidRequest,
            format!(
                "\"{}\" names more than one OpenCode-visible location; toggling one would also change the other: {paths}",
                skill.name
            ),
        ));
    }
    Ok(())
}

/// Sets or clears `permission.skill.<name>` in `~/.config/opencode/
/// opencode.json`. Refuses when only `opencode.jsonc` exists, or when the
/// skill name resolves to more than one OpenCode-visible location
/// ([`refuse_opencode_name_collision`]).
#[allow(clippy::too_many_arguments)]
fn set_opencode_switch(
    rt: &Runtime,
    session: &mut crate::ports::MutationSession,
    fs: &dyn ScopeFs,
    home: &Path,
    skill: &InstalledSkillDto,
    id: &EventId,
    kind: crate::events::EventKind,
    enabled: bool,
) -> Result<(u32, u32), CoreError> {
    refuse_opencode_name_collision(skill)?;
    let config_path = home.join(".config/opencode/opencode.json");
    let jsonc_path = home.join(".config/opencode/opencode.jsonc");
    crate::harness_switch::opencode_refuses_jsonc(
        fs.symlink_metadata(&config_path).is_ok(),
        fs.symlink_metadata(&jsonc_path).is_ok(),
    )?;

    let manifest =
        session
            .store
            .backup_paths(&session.guard, id, std::slice::from_ref(&config_path))?;
    let pre_fingerprint = manifest.entries.first().and_then(|e| e.fingerprint.clone());
    let inverse =
        crate::events::restore_backup_inverse(&config_path, pre_fingerprint.as_ref(), None);
    let draft = crate::events::EventDraft {
        kind,
        skill: skill.name.clone(),
        harness: Some(AgentId::from(AgentId::OPEN_CODE)),
        scope: Some("global".to_string()),
        project_path: None,
        payload: serde_json::json!({ "skill": skill.name.0, "harness": AgentId::OPEN_CODE }),
        inverse: Some(inverse),
        backup_dir: Some(manifest.backup_dir.clone()),
    };
    session.store.record(&session.guard, id, &draft)?;

    // See `set_claude_code_switch`'s matching comment: every fallible step
    // after `record` runs inside this closure so it reaches `finish(Failed)`
    // below, not just the final `write_atomic` call.
    let mutate: Result<Option<Fingerprint>, CoreError> = (|| {
        let existing = match fs.read_capped(
            &config_path,
            crate::harness_switch::HARNESS_CONFIG_MAX_BYTES,
        ) {
            Ok(bytes) => Some(
                String::from_utf8(bytes)
                    .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(&config_path))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(CoreError::io(&config_path, e)),
        };
        let new_text =
            crate::harness_switch::opencode_toggle(existing.as_deref(), &skill.name.0, !enabled)?;
        let config_parent = config_path.parent().unwrap_or(&config_path).to_path_buf();
        ensure_dir_all(rt, session, fs, &config_parent)?;
        let scoped_config = crate::ports::confine(&rt.scope, fs, &config_path)?;
        fs.write_atomic(&session.guard, &scoped_config, new_text.as_bytes())
            .map_err(|e| CoreError::io(&config_path, e))?;
        crate::events::fingerprint_path(fs, &config_path)
    })();

    match mutate {
        Ok(post_fingerprint) => {
            session.store.finish(
                &session.guard,
                id,
                crate::events::EventStatus::Done,
                post_fingerprint,
            )?;
            Ok((1, 1))
        }
        Err(e) => {
            let _ =
                session
                    .store
                    .finish(&session.guard, id, crate::events::EventStatus::Failed, None);
            Err(e)
        }
    }
}

/// pi has no native per-skill switch; this build stands one up as a
/// `disabledSkills` exclusion list under a `skill-studio` key in pi's own
/// `~/.pi/agent/settings.json` (`PiAdapter::config_relative_path`), left
/// alone by pi itself.
#[allow(clippy::too_many_arguments)]
fn set_pi_switch(
    rt: &Runtime,
    session: &mut crate::ports::MutationSession,
    fs: &dyn ScopeFs,
    home: &Path,
    skill: &InstalledSkillDto,
    id: &EventId,
    kind: crate::events::EventKind,
    enabled: bool,
) -> Result<(u32, u32), CoreError> {
    let config_path = home.join(".pi/agent/settings.json");

    let manifest =
        session
            .store
            .backup_paths(&session.guard, id, std::slice::from_ref(&config_path))?;
    let pre_fingerprint = manifest.entries.first().and_then(|e| e.fingerprint.clone());
    let inverse =
        crate::events::restore_backup_inverse(&config_path, pre_fingerprint.as_ref(), None);
    let draft = crate::events::EventDraft {
        kind,
        skill: skill.name.clone(),
        harness: Some(AgentId::from(AgentId::PI)),
        scope: Some("global".to_string()),
        project_path: None,
        payload: serde_json::json!({ "skill": skill.name.0, "harness": AgentId::PI }),
        inverse: Some(inverse),
        backup_dir: Some(manifest.backup_dir.clone()),
    };
    session.store.record(&session.guard, id, &draft)?;

    // See `set_claude_code_switch`'s matching comment: every fallible step
    // after `record` runs inside this closure so it reaches `finish(Failed)`
    // below, not just the final `write_atomic` call.
    let mutate: Result<Option<Fingerprint>, CoreError> = (|| {
        let existing = match fs.read_capped(
            &config_path,
            crate::harness_switch::HARNESS_CONFIG_MAX_BYTES,
        ) {
            Ok(bytes) => Some(
                String::from_utf8(bytes)
                    .map_err(|e| CoreError::new(ErrorCode::Io, e.to_string()).at(&config_path))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(CoreError::io(&config_path, e)),
        };
        let new_text =
            crate::harness_switch::pi_toggle(existing.as_deref(), &skill.name.0, !enabled)?;
        let config_parent = config_path.parent().unwrap_or(&config_path).to_path_buf();
        ensure_dir_all(rt, session, fs, &config_parent)?;
        let scoped_config = crate::ports::confine(&rt.scope, fs, &config_path)?;
        fs.write_atomic(&session.guard, &scoped_config, new_text.as_bytes())
            .map_err(|e| CoreError::io(&config_path, e))?;
        crate::events::fingerprint_path(fs, &config_path)
    })();

    match mutate {
        Ok(post_fingerprint) => {
            session.store.finish(
                &session.guard,
                id,
                crate::events::EventStatus::Done,
                post_fingerprint,
            )?;
            Ok((1, 1))
        }
        Err(e) => {
            let _ =
                session
                    .store
                    .finish(&session.guard, id, crate::events::EventStatus::Failed, None);
            Err(e)
        }
    }
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
            unread_roots: vec![],
            timings: vec![],
        };
        let env = ResultEnvelope::from_result(
            Operation::Scan,
            &scope(),
            &OpContext::uncancellable(CorrelationId("c1".into())),
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
                unread_roots: vec![],
                timings: vec![],
            },
            issues: vec![issue.clone()],
        };
        let mut partial = complete.clone();
        partial.inventory.completeness = Completeness::Partial;
        let ctx = || OpContext::uncancellable(CorrelationId("c3".into()));
        let ok = ResultEnvelope::from_result(Operation::Diagnose, &scope(), &ctx(), Ok(complete));
        assert_eq!(ok.exit_status(), 1);
        let part = ResultEnvelope::from_result(Operation::Diagnose, &scope(), &ctx(), Ok(partial));
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
    fn harnesses_reports_every_first_class_harness_with_a_state_and_evidence_or_names_the_missing_row(
    ) {
        use crate::harness::HarnessCatalog;
        use crate::identity::AgentId;
        use crate::ports::Ports;
        use crate::testing::{
            FakeClock, FakeIds, FakeLease, FakeProcessSpawner, FakeToolLookup, NoHistory,
            RecordingSink,
        };
        use std::sync::Arc;

        // Claude Code resolves on PATH and answers `--version`; every other
        // harness is absent, so its state must fall back to `NotFound` with
        // `Unknown` version/install-method evidence rather than a panic or a
        // missing row.
        let fs = FixtureBuilder::new().dir("/h").build_fs();
        let mut lookup = FakeToolLookup::default();
        lookup.binaries.insert(
            "claude".into(),
            "/usr/local/Cellar/claude/1.2.3/bin/claude".into(),
        );
        let mut spawner = FakeProcessSpawner::default();
        spawner.outputs.insert(
            "/usr/local/Cellar/claude/1.2.3/bin/claude".into(),
            ("claude-code 1.2.3\n".into(), 0),
        );
        let ports = Ports {
            fs: Arc::new(fs),
            clock: Arc::new(FakeClock::at(0)),
            ids: Arc::new(FakeIds::default()),
            leases: Arc::new(FakeLease::default()),
            history: Arc::new(NoHistory),
            sink: Arc::new(RecordingSink::default()),
            spawner: Some(Arc::new(spawner)),
            discovery: None,
            tools: Some(Arc::new(lookup)),
            catalog: Arc::new(HarnessCatalog::builtin()),
        };
        let rt = Runtime::new(&RuntimeScope::fixture("/h"), ports).unwrap();
        let ctx = OpContext::uncancellable(CorrelationId("c6".into()));

        let report = harnesses(&rt, &ctx, &HarnessesRequest {}).unwrap();
        assert_eq!(report.harnesses.len(), 6, "one row per first-class harness");

        let claude = report
            .harnesses
            .iter()
            .find(|row| row.id.as_str() == AgentId::CLAUDE_CODE)
            .expect("claude-code row is missing");
        assert!(claude.executable.is_some());
        assert_eq!(claude.version.value.as_deref(), Some("claude-code 1.2.3"));
        assert!(
            claude.install_method.value.is_some(),
            "a resolved executable path must infer an install method, not stay Unknown"
        );

        for row in report
            .harnesses
            .iter()
            .filter(|r| r.id.as_str() != AgentId::CLAUDE_CODE)
        {
            assert!(
                row.executable.is_none(),
                "{} should not resolve without a binary on PATH",
                row.id.as_str()
            );
            assert!(
                row.version.value.is_none(),
                "{} version must be Unknown",
                row.id.as_str()
            );
            assert!(
                row.install_method.value.is_none(),
                "{} install method must be Unknown",
                row.id.as_str()
            );
        }
    }

    #[test]
    fn busy_lease_exits_3() {
        let env: ResultEnvelope<Inventory> = ResultEnvelope::from_result(
            Operation::Scan,
            &scope(),
            &OpContext::uncancellable(CorrelationId("c2".into())),
            Err(CoreError::new(ErrorCode::ScopeBusy, "held by pid 42")),
        );
        assert_eq!(env.exit_status(), 3);
        assert!(env.data.is_none());
    }

    #[test]
    fn a_hash_budget_stop_does_not_shorten_the_fingerprint_file_list_or_names_the_missing_file() {
        // b.md is bigger than what's left of `max_bytes` after `a.md`, so
        // the hash side truncates there; `c.md` is small again, but must
        // never be reached by the hash side once truncated. The fingerprint
        // side runs on its own MAX_FOLDER_BYTES budget (unaffected by this
        // small `max_bytes`) and so must see all three files regardless.
        let fs = FixtureBuilder::new()
            .dir("/h/skill")
            .file("/h/skill/a.md", &[b'a'; 10])
            .file("/h/skill/b.md", &[b'b'; 20])
            .file("/h/skill/c.md", &[b'c'; 10])
            .build_fs();
        let root = Path::new("/h/skill");
        let mut walk = FactsWalk::default();
        walk_folder_for_facts(
            &fs,
            &OpContext::uncancellable(CorrelationId("facts-walk-test".into())),
            root,
            root,
            25,
            &mut walk,
        )
        .unwrap();

        let hashable: Vec<_> = walk.hashable.iter().map(|f| f.rel_path.clone()).collect();
        assert_eq!(hashable, vec![Path::new("a.md")]);
        assert!(walk.truncated, "hash side must stop once b.md overflows");

        let fingerprint_names: Vec<_> = walk
            .fingerprint_files
            .iter()
            .map(|(rel, _, _)| rel.clone())
            .collect();
        for name in ["a.md", "b.md", "c.md"] {
            assert!(
                fingerprint_names.contains(&PathBuf::from(name)),
                "fingerprint_files is missing {name}, only has {fingerprint_names:?}"
            );
        }
    }

    /// Minimal `DeploymentDto` for `outdated_target` precedence tests - only
    /// `source_kind` and `plugin` matter to that function; every other field
    /// takes a placeholder value no test here reads.
    fn minimal_deployment(
        source_kind: SourceKind,
        plugin: Option<PluginSourceDto>,
    ) -> DeploymentDto {
        DeploymentDto {
            id: DeploymentId::parse("dep:v1/g/-/universal/-/x").unwrap(),
            root: RootRef::new(RootScope::Global, RootKind::Universal).unwrap(),
            harness: None,
            path: PathBuf::from("/h/.agents/skills/x"),
            destination: SkillDestination::Universal,
            backing: BackingRelationship::Independent,
            mutability: DeploymentMutability::ReadOnly,
            link_target: None,
            shared_via_whole_dir_link: false,
            is_symlink: false,
            resolved_path: None,
            symlink_is_broken: false,
            symlink_error: None,
            owner_kind: LifecycleOwnerKind::Copy,
            owner_id: None,
            content_fingerprint: None,
            disabled_by: None,
            disabled_readers: Vec::new(),
            spec_violations: Vec::new(),
            plugin,
            frontmatter: None,
            frontmatter_fields: BTreeMap::new(),
            has_spec: false,
            folder_bytes: 0,
            file_count: 0,
            skill_md_tokens: 0,
            description_tokens: 0,
            content_hash: "hash".to_string(),
            modified_at: None,
            folder_truncated: false,
            in_git_repo: false,
            studio_disabled: false,
            source_kind,
        }
    }

    /// Flow: a skill deployed by both skills-sh and dotagents (the plan's
    /// scan order does not put dotagents first).
    /// Expectation: `outdated_target` classifies it as `SourceKind::Dotagents`,
    /// the higher-precedence method, not whichever deployment is first in
    /// `skill.deployments`.
    /// A failure here means precedence reverted to `deployments.first()`, or
    /// names the wrong method it picked instead.
    #[test]
    fn a_skill_deployed_by_two_methods_is_classified_by_precedence_not_deployment_order_or_names_the_method_it_picked(
    ) {
        let skill = InstalledSkillDto {
            name: SkillName("write-tests".to_string()),
            description: None,
            deployments: vec![
                minimal_deployment(SourceKind::SkillsSh, None),
                minimal_deployment(SourceKind::Dotagents, None),
            ],
        };
        let target = outdated_target(&skill).expect("a deployed skill always yields a target");
        assert_eq!(target.source_kind, SourceKind::Dotagents);
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
        fn scan_never_spawns_a_probe_even_when_a_spawner_port_is_wired() {
            use crate::testing::PanicOnSpawn;

            let fs = FixtureBuilder::new()
                .dir("/h/.claude/skills/write-tests")
                .file(
                    "/h/.claude/skills/write-tests/SKILL.md",
                    b"---\nname: write-tests\ndescription: Writes tests.\n---\nBody.",
                )
                .build_fs();
            let ports = Ports {
                fs: Arc::new(fs),
                clock: Arc::new(FakeClock::at(0)),
                ids: Arc::new(FakeIds::default()),
                leases: Arc::new(FakeLease::default()),
                history: Arc::new(NoHistory),
                sink: Arc::new(RecordingSink::default()),
                spawner: Some(Arc::new(PanicOnSpawn)),
                discovery: None,
                tools: None,
                catalog: Arc::new(HarnessCatalog::builtin()),
            };
            let rt = Runtime::new(&RuntimeScope::fixture("/h"), ports).unwrap();
            let inv = scan(&rt, &ctx(), &ScanRequest::default()).unwrap();
            assert_eq!(inv.skills.len(), 1, "scan must still find the skill");
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
                // Call 0 is `scan_inner`'s `start` read. Calls 1-3 are its
                // own timing instrumentation, read before the roots walk
                // begins: the `ledgers_read` step's start and its
                // `timing::step` reading, then the `roots_walk` step's
                // start. Calls (1 + PRELUDE_CALLS)..=(within_budget_calls +
                // PRELUDE_CALLS) are every call the global groups make
                // walking their roots and timing `dir_walk`/`skill_md_read`/
                // `frontmatter_parse`/`plugin_cache_walk`.
                const PRELUDE_CALLS: u64 = 3;
                let call = self.calls.fetch_add(1, Ordering::SeqCst);
                if call <= self.within_budget_calls + PRELUDE_CALLS {
                    Duration::from_millis(0)
                } else {
                    Duration::from_millis(3_000)
                }
            }
        }

        /// Never trips a budget, just counts `monotonic()` calls - used to
        /// measure exactly how many clock reads one `scan` makes walking a
        /// fixture's global roots, so [`BudgetAfterNClock`] can be
        /// calibrated to trip right at the project-scope boundary without a
        /// hand-counted constant that would silently go stale the next time
        /// `scan_inner`'s instrumentation changes.
        struct CountingClock {
            calls: AtomicU64,
        }

        impl Clock for CountingClock {
            fn now(&self) -> chrono::DateTime<Utc> {
                Utc::now()
            }

            fn monotonic(&self) -> Duration {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Duration::from_millis(0)
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

            // Run the same fixture with no project roots at all through a
            // clock that never trips, so every clock read comes from
            // `scan_inner`'s own timing (`ledgers_read`, `roots_walk`, and
            // the per-section reads inside it) plus the global groups' own
            // walk - never from a project group, since there are none to
            // walk. Subtracting the fixed pre/post-loop reads (4 before the
            // loop, 6 after it: `roots_walk`'s own elapsed read, the two
            // post-loop steps' start+elapsed reads, and the whole-call
            // `op_timing` read) leaves exactly the call count the global
            // groups' walk consumed, which is what the calibrated clock
            // below needs to trip the budget right at the project-scope
            // boundary without a guessed constant that would go stale the
            // next time `scan_inner`'s instrumentation changes.
            let mut probe_scope = scope.clone();
            probe_scope.projects = ProjectSelection::Explicit { paths: Vec::new() };
            let counting_clock = Arc::new(CountingClock {
                calls: AtomicU64::new(0),
            });
            let probe_rt = Runtime::new(
                &probe_scope,
                ports_for(counting_clock.clone() as Arc<dyn Clock>),
            )
            .unwrap();
            scan(&probe_rt, &ctx(), &ScanRequest::default()).unwrap();
            const PRELUDE_CALLS: u64 = 4;
            const POSTLUDE_CALLS: u64 = 6;
            let within_budget_calls =
                counting_clock.calls.load(Ordering::SeqCst) - PRELUDE_CALLS - POSTLUDE_CALLS;

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

        /// A scan under a readable root whose one skill has an unreadable
        /// `SKILL.md` puts that skill's own directory in `unread_roots`,
        /// not the whole root. Fails if `unread_roots` stays empty (the
        /// desktop merge in `skill_refresh.rs` would then drop that
        /// skill's previous row instead of carrying it over) or if it
        /// contains the root instead of the narrower skill directory
        /// (which would carry over every sibling skill too).
        #[test]
        fn unreadable_skill_md_under_a_readable_root_scopes_unread_roots_to_that_skill_dir() {
            use crate::testing::FailingFs;

            let fs = FixtureBuilder::new()
                .dir("/h/.claude/skills/good-skill")
                .file(
                    "/h/.claude/skills/good-skill/SKILL.md",
                    b"---\nname: good-skill\ndescription: Fine.\n---\n",
                )
                .dir("/h/.claude/skills/broken-skill")
                .file("/h/.claude/skills/broken-skill/SKILL.md", b"---\n---\n")
                .build_fs();
            let failing = FailingFs::wrap(Arc::new(fs));
            failing.fail_read_prefix_for(PathBuf::from("/h/.claude/skills/broken-skill/SKILL.md"));
            let ports = Ports {
                fs: Arc::new(failing),
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
            let rt = Runtime::new(&RuntimeScope::fixture("/h"), ports).unwrap();
            let inv = scan(&rt, &ctx(), &ScanRequest::default()).unwrap();

            assert_eq!(inv.completeness, Completeness::Partial);
            assert_eq!(
                inv.unread_roots,
                vec![PathBuf::from("/h/.claude/skills/broken-skill")]
            );
            assert_eq!(
                inv.skills.len(),
                1,
                "the readable skill must still be found"
            );
            assert_eq!(inv.skills[0].name.0, "good-skill");
        }
    }
}
