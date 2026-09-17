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
