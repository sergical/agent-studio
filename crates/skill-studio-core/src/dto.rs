//! Data transfer types shared by every surface.
//!
//! Every type here derives `serde` and `JsonSchema`. TypeScript types and MCP
//! tool schemas are generated from these definitions; nothing is hand-written
//! on the other side.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::frontmatter::SkillFrontmatter;
use crate::harness::DisabledBy;
use crate::identity::{
    AgentId, BackingRelationship, DeploymentId, DeploymentMutability, EventId, Fingerprint,
    LifecycleOwnerKind, OwnerId, ProposalId, RootRef, RootScope, SkillDestination, SkillName,
    SourceKind,
};

/// How serious an issue is.
///
/// Invariant: `Error` blocks a mutation on the deployment; `Warning` does
/// not; `Off` is informational (the deployment is intentionally disabled).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    /// Informational.
    Off,
    /// Should be fixed.
    Warning,
    /// Must be fixed before the deployment can be changed.
    Error,
}

/// Structured issue kind. New kinds may be added; names never change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    /// A symlink whose target is missing.
    BrokenLink,
    /// A link Skill Studio could not read.
    UnreadableLink,
    /// A spec rule is violated (message says which).
    SpecViolation,
    /// Frontmatter that a repair proposal can fix.
    RepairableFrontmatter,
    /// Two deployments of one name have different bytes.
    Drift,
    /// The same name is installed twice in one scope.
    Duplicate,
    /// The deployment is parked.
    Parked,
    /// The deployment is disabled for a reader.
    Disabled,
    /// A root could not be read inside the budget.
    RootUnreadable,
}

/// What the user can do about an issue.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "action")]
pub enum NextAction {
    /// Run `preview_frontmatter_repair` on the deployment.
    PreviewRepair {
        /// Target.
        deployment_id: DeploymentId,
    },
    /// Remove or relink the broken link.
    RepairLink {
        /// Target.
        deployment_id: DeploymentId,
    },
    /// Restore the parked or disabled deployment.
    Restore {
        /// Target.
        deployment_id: DeploymentId,
    },
    /// Rescan with a longer read budget.
    Rescan,
    /// No automatic action exists.
    None,
}

/// One diagnosed problem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Issue {
    /// Kind.
    pub kind: IssueKind,
    /// Severity.
    pub severity: Severity,
    /// Skill the issue belongs to.
    pub skill: SkillName,
    /// Deployment the issue belongs to, when it is deployment-specific.
    pub deployment_id: Option<DeploymentId>,
    /// Message for a person.
    pub message: String,
    /// Suggested action.
    pub next_action: NextAction,
}

/// Whether a scan saw everything it was asked to see.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Completeness {
    /// Every root was read.
    Complete,
    /// Some roots were skipped; see `observations`.
    Partial,
}

/// Something worth telling the caller that is not an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Observation {
    /// Root the observation is about, when there is one.
    pub root: Option<RootRef>,
    /// Message for a person.
    pub message: String,
}

/// How long one phase took.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Timing {
    /// Phase name.
    pub phase: String,
    /// Elapsed milliseconds.
    pub elapsed_ms: u64,
}

/// Where a plugin-cache deployment came from, per the agent-plugins.org
/// manifest convention (`<cache>/<marketplace>/<plugin>/<version>/skills/`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PluginSourceDto {
    /// Marketplace directory name.
    pub marketplace: String,
    /// Plugin directory name.
    pub plugin: String,
    /// Version directory name, when the layout has one.
    pub version: Option<String>,
    /// Claude Code's `enabledPlugins["<plugin>@<marketplace>"]` state:
    /// `Some(false)` when the harness has the plugin switched off,
    /// `Some(true)` when switched on, `None` when the harness records
    /// nothing for it (including every non-Claude-Code harness).
    pub enabled: Option<bool>,
}

/// One installed copy or link of a skill.
///
/// Invariant: `harness` is an [`AgentId`] or `None` for the universal and
/// parked roots; it is never a display string. `plugin` is `Some` exactly
/// when `root.kind` is [`crate::identity::RootKind::PluginCache`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeploymentDto {
    /// Opaque id.
    pub id: DeploymentId,
    /// Root the deployment lives in.
    pub root: RootRef,
    /// Harness that owns the root, when it is a harness root.
    pub harness: Option<AgentId>,
    /// Absolute lexical path of the skill directory.
    pub path: PathBuf,
    /// Universal or per-harness.
    pub destination: SkillDestination,
    /// How the bytes are backed.
    pub backing: BackingRelationship,
    /// Whether writes are allowed.
    pub mutability: DeploymentMutability,
    /// Link target, when the deployment is a symlink.
    pub link_target: Option<PathBuf>,
    /// True when the skills root itself is a link into the universal root.
    pub shared_via_whole_dir_link: bool,
    /// True when the deployment's own directory entry is a symlink.
    pub is_symlink: bool,
    /// Canonicalized directory the deployment resolves to, when that is
    /// meaningfully different from `path`. Distinct from `link_target`:
    /// `link_target` is the symlink's own target, while `resolved_path` is
    /// `None` for an ordinary directory that canonicalizes to itself or for
    /// a symlink whose target does not resolve.
    pub resolved_path: Option<PathBuf>,
    /// True when the deployment is a symlink whose target does not resolve.
    pub symlink_is_broken: bool,
    /// Error reading the symlink target, when one occurred.
    pub symlink_error: Option<String>,
    /// Which ledger owns the lifecycle.
    pub owner_kind: LifecycleOwnerKind,
    /// Owner id, when a ledger owns it.
    pub owner_id: Option<OwnerId>,
    /// Content fingerprint over the directory tree.
    pub content_fingerprint: Option<Fingerprint>,
    /// Why the deployment is off, when it is.
    pub disabled_by: Option<DisabledBy>,
    /// Readers of a universal deployment that have it disabled.
    pub disabled_readers: Vec<AgentId>,
    /// Spec violations, verbatim messages.
    pub spec_violations: Vec<String>,
    /// Plugin provenance for a plugin-cache deployment.
    pub plugin: Option<PluginSourceDto>,
    /// Parsed `SKILL.md` frontmatter, when the file parsed.
    pub frontmatter: Option<SkillFrontmatter>,
    /// Every top-level frontmatter key, stringified.
    pub frontmatter_fields: std::collections::BTreeMap<String, String>,
    /// True when the SKILL.md matches the getsentry/skillet spec pattern.
    pub has_spec: bool,
    /// Total bytes read while walking the skill folder.
    pub folder_bytes: u64,
    /// Total files counted while walking the skill folder.
    pub file_count: u32,
    /// Token count of the whole `SKILL.md` file.
    pub skill_md_tokens: u32,
    /// Token count of just `"{name}: {description}"` - the prompt cost the
    /// model actually pays per turn, as opposed to `skill_md_tokens` which
    /// counts the whole file.
    pub description_tokens: u32,
    /// sha256 over the sorted (relative path, bytes) pairs of the skill
    /// folder. Distinct from `content_fingerprint`: a different, whole-folder
    /// scheme kept for parity with the desktop's `SkillCandidate`.
    pub content_hash: String,
    /// RFC3339 of the newest file mtime in the skill folder.
    pub modified_at: Option<DateTime<Utc>>,
    /// True when the folder walk hit the file-count or byte cap and stopped
    /// early - `folder_bytes`/`file_count`/`content_hash` are partial.
    pub folder_truncated: bool,
    /// True when the skill's directory sits inside a git working tree (a
    /// `.git` file or directory on some ancestor). Distinguishes a
    /// version-controlled directory from a genuinely unmanaged one - see
    /// [`SourceKind::InRepo`].
    pub in_git_repo: bool,
    /// True when this deployment was found inside a root's
    /// `.skill-studio-disabled/` holding directory. Maps to `disabled_by ==
    /// Some(DisabledBy::StudioMoved)`.
    pub studio_disabled: bool,
    /// Provenance classification, per `apps/desktop`'s
    /// `provenance::classify_source_kind`.
    pub source_kind: SourceKind,
}

/// One skill with all of its deployments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstalledSkillDto {
    /// Name.
    pub name: SkillName,
    /// Description from the first readable deployment.
    pub description: Option<String>,
    /// Deployments in scan order.
    pub deployments: Vec<DeploymentDto>,
}

/// Result of `scan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Inventory {
    /// Skills sorted by name.
    pub skills: Vec<InstalledSkillDto>,
    /// Projects that were covered, sorted by canonical path.
    pub projects: Vec<PathBuf>,
    /// Whether every root was read.
    pub completeness: Completeness,
    /// Notes about skipped roots and other facts.
    pub observations: Vec<Observation>,
    /// Every path prefix this run could not read: a whole root (a budget
    /// overrun before the root was reached, or a `read_dir` error on the
    /// root itself), or a single skill directory whose `SKILL.md` was
    /// unreadable within an otherwise-readable root - the set a caller
    /// merging this result into a previous one must scope a carried-over
    /// deployment to.
    #[serde(default)]
    pub unread_roots: Vec<PathBuf>,
    /// Phase timings.
    pub timings: Vec<Timing>,
}

/// Request for `scan`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ScanRequest {
    /// Restrict to these skill names; empty means all.
    pub skills: Vec<SkillName>,
    /// Include per-phase timings.
    pub timings: bool,
}

/// Result of `diagnose`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Diagnosis {
    /// The inventory the issues were derived from.
    pub inventory: Inventory,
    /// Issues sorted by severity, then skill, then kind.
    pub issues: Vec<Issue>,
}

/// Request for `capabilities`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct CapabilitiesRequest {
    /// Restrict to these harnesses; empty means all. A name without a
    /// catalog row is `invalid_request`.
    pub harnesses: Vec<AgentId>,
    /// Also probe the machine (config presence, runner binary).
    pub observe: bool,
    /// Executables to look up on `PATH` (`npx`, `dotagents`, `gh`). Needs
    /// the `ToolLookup` port; without it a non-empty list is `unsupported`.
    pub tools: Vec<String>,
}

/// Request for `harnesses`. Empty today; kept as a struct rather than `()`
/// so a future per-harness filter matches `CapabilitiesRequest`'s shape
/// without a wire break.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct HarnessesRequest {}

/// Ways a frontmatter repair may be applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum RepairApplyMode {
    /// Rewrite the file in place.
    ApplyFix,
    /// Rewrite the installed copy that a ledger owns.
    FixInstalledCopy,
    /// Fork first, then rewrite the fork.
    ForkAndFix,
}

/// A proposed frontmatter repair.
///
/// Invariant: `apply_frontmatter_repair` refuses the proposal when the file
/// no longer matches `expected_fingerprint` or the owner changed. The repair
/// is deterministic: apply recomputes the proposal from the bytes on disk
/// and compares `proposal_id`, so the caller's `proposed_content` is shown,
/// never written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FrontmatterRepairPreview {
    /// Proposal id.
    pub proposal_id: ProposalId,
    /// Target deployment.
    pub deployment_id: DeploymentId,
    /// Absolute path of `SKILL.md`.
    pub path: PathBuf,
    /// Scope of the deployment's root.
    pub scope: RootScope,
    /// Why the repair is proposed, for a person (the spec rule that fails).
    pub reason: String,
    /// Owner at preview time.
    pub owner_id: Option<OwnerId>,
    /// Owner kind at preview time.
    pub owner_kind: LifecycleOwnerKind,
    /// Fingerprint of the current `SKILL.md` bytes.
    pub expected_fingerprint: Fingerprint,
    /// Fingerprint of the proposed bytes.
    pub proposed_fingerprint: Fingerprint,
    /// Current `SKILL.md` text, for a side-by-side view.
    pub original_content: String,
    /// Proposed `SKILL.md` text, for a side-by-side view.
    pub proposed_content: String,
    /// Unified diff for a person.
    pub diff: String,
    /// Modes the caller may choose from.
    pub allowed_apply_modes: Vec<RepairApplyMode>,
    /// Warning shown for [`RepairApplyMode::FixInstalledCopy`]: the next
    /// managed update overwrites the fix.
    pub managed_update_warning: Option<String>,
}

/// Request for `preview_frontmatter_repair`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepairPreviewRequest {
    /// Target deployment.
    pub deployment_id: DeploymentId,
}

/// Request for `apply_frontmatter_repair`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RepairApplyRequest {
    /// The preview to apply, unchanged.
    pub preview: FrontmatterRepairPreview,
    /// Chosen mode; must be in `preview.allowed_apply_modes`.
    pub mode: RepairApplyMode,
}

/// Result of `apply_frontmatter_repair`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "outcome")]
pub enum RepairOutcome {
    /// The file was rewritten and an event recorded.
    Applied {
        /// History event.
        event_id: EventId,
        /// Deployment written (the fork for `ForkAndFix`).
        deployment_id: DeploymentId,
    },
    /// The file already had the proposed bytes; nothing was written.
    AlreadyApplied {
        /// Deployment inspected.
        deployment_id: DeploymentId,
    },
}

/// Request for `fix_skill`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FixSkillRequest {
    /// Skill to run the doctor checks and repairs for.
    pub skill: SkillName,
}

/// One repair `fix_skill` applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum FixApplied {
    /// A frontmatter repair was written, the same write
    /// `apply_frontmatter_repair` performs.
    FrontmatterRepair {
        /// Deployment written.
        deployment_id: DeploymentId,
        /// History event.
        event_id: EventId,
    },
}

/// One issue `fix_skill` found but could not repair, named with its path so
/// the caller can show it rather than a generic toast.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnrepairedIssue {
    /// Path of the offending file or folder, when the issue names one.
    pub path: PathBuf,
    /// Message for a person.
    pub message: String,
}

/// One pair of differing copies: never merged, named for the caller to open
/// side by side in the user's editor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConflictSummary {
    /// Skill the conflict belongs to.
    pub skill: SkillName,
    /// One-line summary for a person.
    pub message: String,
    /// First copy's path.
    pub path_a: PathBuf,
    /// Second copy's path.
    pub path_b: PathBuf,
}

/// Result of `fix_skill`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FixSkillOutcome {
    /// Skill the fix ran for.
    pub skill: SkillName,
    /// Repairs written.
    pub applied: Vec<FixApplied>,
    /// Issues named but not repaired, with their paths.
    pub unrepaired: Vec<UnrepairedIssue>,
    /// Conflicts found; the app opens both paths in the user's editor.
    pub conflicts: Vec<ConflictSummary>,
}

/// Request for `diagnose_conflict`. Empty: a conflict is a relationship
/// between two deployments of one skill, found by scanning the whole
/// inventory rather than named one deployment at a time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct DiagnoseConflictRequest {}

/// Result of `diagnose_conflict`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ConflictReport {
    /// Every conflict found. Read-only: nothing is written.
    pub conflicts: Vec<ConflictSummary>,
}

/// Whether a history event can be restored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "restorable")]
pub enum RestoreCapability {
    /// Restore is available.
    Yes,
    /// Already reverted by the named event.
    Reverted {
        /// The restore event.
        by: EventId,
    },
    /// The row has no inverse.
    NoInverse,
    /// The row's kind is not known to this version of the core.
    UnknownKind,
    /// `pending`, or `failed`/`interrupted` without a `restore_backup`
    /// inverse and a `backup_dir`: the row's inverse never moved what it
    /// describes, or there is no backup for a drift-checked restore to
    /// apply, so applying it would act on live state the row does not own.
    NotCompleted {
        /// The row's status, so a caller can explain the refusal.
        status: String,
    },
}

/// Whether the files an event touched still hold the bytes it recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DriftState {
    /// `list_events` did not compare fingerprints (the row has no inverse,
    /// or the caller did not ask).
    Unchecked,
    /// Live fingerprints match the recorded post-mutation fingerprints.
    Clean,
    /// At least one path changed since the event; restore needs `force`.
    Drifted,
}

/// History row projected for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EventDto {
    /// Id.
    pub id: EventId,
    /// Time.
    pub ts: DateTime<Utc>,
    /// Kind, verbatim from the row.
    pub kind: String,
    /// Skill.
    pub skill: SkillName,
    /// Harness, when harness-scoped.
    pub harness: Option<AgentId>,
    /// `global` or `project`.
    pub scope: Option<String>,
    /// Project path, when project-scoped.
    pub project_path: Option<PathBuf>,
    /// `pending`, `done`, `failed`, or `interrupted`.
    pub status: String,
    /// Whether restore is possible.
    pub restore: RestoreCapability,
    /// Whether the touched files still match the recorded fingerprints.
    pub drift: DriftState,
    /// Relative backup directory, when bytes were preserved.
    pub backup_dir: Option<String>,
}

/// Request for `list_events`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ListEventsRequest {
    /// Restrict to one skill.
    pub skill: Option<SkillName>,
    /// Maximum rows; `0` means the core default.
    pub limit: u32,
    /// Pagination cursor: return only rows older than this event. Pass the
    /// last id of the previous page. Ids are ULIDs, so "older" is "sorts
    /// before".
    pub after: Option<EventId>,
    /// Compare live fingerprints with the recorded ones and fill `drift`.
    /// Costs one read per touched path; `false` leaves `Unchecked`.
    pub check_drift: bool,
}

/// Request for `restore_event`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestoreRequest {
    /// Event to revert.
    pub event_id: EventId,
    /// Proceed on drift; the drifted bytes are backed up first.
    #[serde(default)]
    pub force: bool,
}

/// Result of `restore_event`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RestoreOutcome {
    /// The new `restore` event.
    pub restore_event_id: EventId,
    /// The event that was reverted.
    pub reverted_event_id: EventId,
    /// Paths put back.
    pub restored_paths: Vec<PathBuf>,
}

/// One command's health over the rollup window (see [`crate::health::health_rollup`]):
/// how often it ran, how often it failed, and how long it took.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CommandHealth {
    /// The command name, as recorded in `timing.jsonl`.
    pub command: String,
    /// Calls within the window.
    pub count: u64,
    /// Calls within the window whose outcome was `"error"`.
    pub failures: u64,
    /// Median elapsed milliseconds, nearest-rank.
    pub p50_ms: u64,
    /// 95th-percentile elapsed milliseconds, nearest-rank.
    pub p95_ms: u64,
    /// The most recent failing call's error text, if any failed.
    pub last_error: Option<String>,
}

/// Request to park one universal deployment: remove its per-harness links
/// and move its directory into the parked root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParkRequest {
    /// The universal deployment to park.
    pub deployment_id: DeploymentId,
}

/// Result of `park`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ParkOutcome {
    /// The `park` event.
    pub event_id: EventId,
    /// The deployment that was parked.
    pub deployment_id: DeploymentId,
    /// Where the directory now lives, under the parked root.
    pub parked_path: PathBuf,
}

/// Request to unpark one deployment: move its directory back to the
/// universal root and recreate any per-harness link it had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnparkRequest {
    /// The parked deployment to restore.
    pub deployment_id: DeploymentId,
}

/// Result of `unpark`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnparkOutcome {
    /// The `unpark` event.
    pub event_id: EventId,
    /// The deployment that was restored.
    pub deployment_id: DeploymentId,
    /// Where the directory now lives, under the universal root.
    pub restored_path: PathBuf,
}

/// Request to turn a skill's per-harness native switch on or off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetHarnessEnabledRequest {
    /// The skill to toggle.
    pub skill: SkillName,
    /// The harness whose native switch to flip.
    pub harness: AgentId,
    /// `true` enables the skill for this harness; `false` disables it.
    pub enabled: bool,
    /// The project the targeted row is scoped to; `None` for the global row.
    /// Claude Code's switch is a per-scope symlink slot
    /// (`<project>/.claude/skills/<name>` vs `<home>/.claude/skills/<name>`),
    /// so this picks which slot a project-scoped skill's toggle touches.
    /// Every other harness's switch is keyed by name alone and ignores it.
    #[serde(default)]
    pub project_path: Option<PathBuf>,
}

/// Result of `set_harness_enabled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SetHarnessEnabledOutcome {
    /// The `set_harness_enabled` event.
    pub event_id: EventId,
    /// The skill that was toggled.
    pub skill: SkillName,
    /// The harness whose switch was flipped.
    pub harness: AgentId,
    /// How many of the harness's paths for this skill were toggled before
    /// either finishing or hitting a failure.
    pub toggled: u32,
    /// How many paths the harness's switch needed to touch in total.
    pub total: u32,
}

/// Which of the three ways `ops::install` can put a skill's bytes on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstallMethod {
    /// Fetches or copies bytes and stages/swaps them into place directly -
    /// no external CLI.
    Copy,
    /// `npx -y @sentry/dotagents add <source>`.
    Dotagents,
    /// `npx -y skills add <source>`.
    SkillsSh,
}

/// One file `InstallMethod::Copy` stages into the new skill's folder, path
/// relative to the folder root (for example `SKILL.md` or
/// `scripts/run.sh`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallFile {
    /// Path relative to the skill folder's own root.
    pub relative_path: PathBuf,
    /// The file's bytes.
    pub contents: Vec<u8>,
}

/// Request to install one skill by [`InstallMethod::Copy`], `Dotagents`, or
/// `SkillsSh`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallRequest {
    /// The folder name the skill will be installed under, both at
    /// `.agents/skills/<name>` and, for `Dotagents`/`SkillsSh`, the name the
    /// CLI is expected to create.
    pub skill: SkillName,
    /// Which method writes the bytes.
    pub method: InstallMethod,
    /// `Global` installs under the scope home; `Project` installs under one
    /// project.
    pub scope: RootScope,
    /// Harnesses to link the new skill into right after install. Only
    /// Claude Code has a per-skill link this build writes; other harnesses
    /// read the universal root directly.
    #[serde(default)]
    pub harnesses: Vec<AgentId>,
    /// `Copy` only: the folder's files, staged then swapped into place.
    #[serde(default)]
    pub files: Vec<InstallFile>,
    /// `Dotagents`/`SkillsSh` only: the source argument passed to the CLI's
    /// `add` command (for example `owner/repo`).
    #[serde(default)]
    pub source: Option<String>,
    /// The source's normalized repository identity, for the trust check
    /// (`None` for a source the policy never gates, like a local path).
    #[serde(default)]
    pub trust_identity: Option<String>,
    /// Confirms the trust prompt for `trust_identity`. A first call with
    /// this `false` against an untrusted identity returns
    /// [`InstallOutcome::NeedsTrust`] instead of writing.
    #[serde(default)]
    pub trust_confirmed: bool,
    /// Saves this call's method and harnesses as the preference
    /// `install_preferences` returns for the next install. Defaults to
    /// `true` so a caller opts out, not in.
    #[serde(default = "default_true")]
    pub save_as_preference: bool,
}

fn default_true() -> bool {
    true
}

/// Result of `install`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InstallOutcome {
    /// The skill was installed.
    Installed {
        /// The `install` event.
        event_id: EventId,
        /// The skill that was installed.
        skill: SkillName,
        /// Where its canonical folder now lives.
        deployment_path: PathBuf,
        /// Harnesses actually linked (a subset of the request's
        /// `harnesses` - only Claude Code gets a link this build).
        linked_harnesses: Vec<AgentId>,
    },
    /// `trust_identity` was set, is not yet trusted, and `trust_confirmed`
    /// was `false`. Nothing was written; retry with `trust_confirmed: true`
    /// once the user confirms.
    NeedsTrust {
        /// The normalized identity that needs confirming.
        identity: String,
    },
}

/// The method and harnesses the last successful install saved, or the
/// environment default when nothing has been saved yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InstallPreferences {
    /// The saved or defaulted method.
    pub method: InstallMethod,
    /// The saved or defaulted harnesses.
    pub harnesses: Vec<AgentId>,
    /// `false` when `method`/`harnesses` are an environment default rather
    /// than a saved preference (no install has completed on this scope
    /// yet).
    pub saved: bool,
}

/// Request to refresh one already-installed skill in place, via
/// `ops::update`. Reuses [`InstallMethod`]: `Copy` re-stages fresh `files`
/// and swaps them in, quarantining the previous tree; `Dotagents`/`SkillsSh`
/// re-run the same CLI `install` shelled out to, in place over the existing
/// destination.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateRequest {
    /// The already-installed skill's folder name.
    pub skill: SkillName,
    /// Which method wrote the deployment being refreshed.
    pub method: InstallMethod,
    /// `Global` updates the scope home; `Project` updates one project.
    pub scope: RootScope,
    /// `Copy` only: the fresh files to stage and swap in.
    #[serde(default)]
    pub files: Vec<InstallFile>,
    /// `Dotagents`/`SkillsSh` only: the source argument the CLI's `add`
    /// command needs to re-fetch (`skills update` itself only takes the
    /// name; `dotagents`' argv still needs the original source).
    #[serde(default)]
    pub source: Option<String>,
    /// `Dotagents` only: an already-resolved commit for a pinned
    /// (`declared_ref`) ledger entry - the caller's own concern, not
    /// re-derived here (see `ops_update`'s module doc).
    #[serde(default)]
    pub ref_pin: Option<String>,
}

/// Result of `update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateOutcome {
    /// The `update` event.
    pub event_id: EventId,
    /// The skill that was updated.
    pub skill: SkillName,
    /// Where its canonical folder lives.
    pub deployment_path: PathBuf,
    /// The tree's git tree SHA before this update.
    pub tree_hash_before: String,
    /// The tree's git tree SHA after this update.
    pub tree_hash_after: String,
}

/// One skill's result inside an `update_all` batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateAllItem {
    /// The skill this result is for.
    pub skill: SkillName,
    /// `Some` on success, `None` when this skill's update failed - the
    /// failure's message is the matching entry in
    /// [`UpdateAllOutcome::errors`].
    pub outcome: Option<UpdateOutcome>,
}

/// Result of `update_all`: one [`UpdateAllItem`] per requested skill, in the
/// order each one finished (not the order requested), plus the message for
/// any that failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateAllOutcome {
    /// One entry per skill `update_all` was asked to refresh.
    pub items: Vec<UpdateAllItem>,
    /// `skill.0` -> error message, for every item whose `outcome` is `None`.
    #[serde(default)]
    pub errors: std::collections::BTreeMap<String, String>,
}

/// Request wrapper for `ops::update_all` - the op itself takes a plain
/// `&[UpdateRequest]`; this only exists so an MCP tool has one schema to
/// declare instead of an array-of-objects root schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UpdateAllRequest {
    /// One request per skill to refresh, each its own journal entry.
    pub requests: Vec<UpdateRequest>,
}
