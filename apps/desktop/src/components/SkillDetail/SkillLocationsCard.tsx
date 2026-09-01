// ============================================================================
// SkillLocationsCard - "Where it lives": one row per deployment (harness,
// relation, path, an Enabled switch where the harness supports it, and a
// Reveal button), plus an Invocation footer row. Replaces the old
// SkillDetailDetails Locations/Invocation sections and the raw frontmatter
// key list.
// ============================================================================

import { Fragment, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { AlertTriangle, ChevronRight, FolderOpen, Link2 } from "lucide-react";
import {
  agentIdFromDeploymentLabel,
  AGENTS_READING_SHARED_ROOT,
  deploymentLinkKind,
  deploymentLinkTarget,
  driftingCopies,
  groupDeploymentsForDisplay,
  locationSummary,
} from "@skill-studio/lib";
import type { AgentId, DeploymentGroup } from "@skill-studio/lib";
import {
  distributeSkillFromShared,
  forkSkill,
  openSkillPath,
  setDeploymentEnabled,
  setHarnessEnabled,
  setSharedHarnessSkillEnabled,
  setSkillInvocation,
} from "../../lib/skill-api";
import { FIRST_CLASS_AGENTS } from "@skill-studio/lib";
import { homeRelativePath } from "@skill-studio/lib";
import type { Deployment, InstalledSkill, InvocationPolicy } from "@skill-studio/lib";
import { useAppStore } from "../../store/appStore";
import { HarnessIcon, harnessIdFromLabel } from "../ui/HarnessIcon";
import { TooltipControl } from "../ui/TooltipControl";
import { SwitchControl } from "../ui/SwitchControl";

interface SkillLocationsCardProps {
  skill: InstalledSkill;
  /** The SKILL.md path `setSkillInvocation` should rewrite - unset when the skill has no own deployment. */
  skillMdPath?: string;
  /** The deployment `skillMdPath` came from, for the Fork-and-save flow's `forkSkill(name, path)` call. */
  skillMdDeployment?: Deployment;
  /** Opens `SkillCompareDialog` - shown as a "Compare copies" header button only when `driftingCopies` finds something. */
  onCompareCopies?: () => void;
}

const INVOCATION_POLICY_OPTIONS: { value: InvocationPolicy; label: string }[] = [
  { value: "both", label: "Both" },
  { value: "user-only", label: "User only" },
  { value: "model-only", label: "Model only" },
];

/** "Both: you can call /skill-name and the model can pick it", etc. */
function invocationPolicyExplanation(policy: InvocationPolicy, skillName: string): string {
  switch (policy) {
    case "both":
      return `Both: you can call /${skillName} and the model can pick it`;
    case "user-only":
      return `User only: only /${skillName} starts it`;
    case "model-only":
      return `Model only: the model picks it; no /${skillName}`;
  }
}

/** Harnesses with a per-skill disable switch - see `skill_harness_disable.rs`. */
const HARNESSES_WITH_PER_SKILL_DISABLE = ["codex", "open-code", "claude-code"];

/**
 * Whether the Enabled switch can actually change this deployment. The disable
 * mechanisms are global: Codex config, OpenCode permission, Claude Code's
 * global per-skill symlink. A project-scope copy has nothing to toggle, so
 * showing the switch there just produces an error - except when the row is
 * already disabled, which must stay re-enableable.
 */
function canToggleHarness(deployment: Deployment): boolean {
  const id = agentIdFromDeploymentLabel(deployment.agent) ?? "";
  if (!HARNESSES_WITH_PER_SKILL_DISABLE.includes(id)) return false;
  if (deployment.disabled) return true;
  if (deployment.scope !== "global") return false;
  return id !== "claude-code" || deployment.is_symlink;
}

/**
 * Whether `setDeploymentEnabled`'s universal move-aside disable applies to
 * this row - every harness row except the shared folder itself and
 * plugin-provided deployments, which `skill_harness_disable.rs` refuses to
 * move (a shared-root move would disable the skill everywhere at once, and a
 * plugin's skill dir belongs to the plugin cache, not Skill Studio).
 */
function canMoveAsideDisable(deployment: Deployment): boolean {
  return deployment.agent !== "shared" && !deployment.plugin && Boolean(deployment.path);
}

/** Plain-text relation for a deployment with no link to show - a chip covers every other case, see `linkChipFor`. */
function relationLabel(deployment: Deployment): string | null {
  switch (deploymentLinkKind(deployment)) {
    case "shared-root":
      // The row's own name is already "Shared folder" - no need to repeat it here.
      return "source of truth";
    case "own":
      return deployment.is_symlink ? null : "copy";
    default:
      return null;
  }
}

/** One row's link relation, as a chip - see the readability rules in spec-ux-4.md section 4. `null` when the deployment isn't a link at all (the shared root itself, or a plain own-directory copy). */
function linkChipFor(
  deployment: Deployment,
): { label: string; target: string | undefined; broken: boolean } | null {
  const kind = deploymentLinkKind(deployment);
  if (kind === "broken") {
    return { label: "broken link", target: deployment.symlink_target, broken: true };
  }
  if (kind === "linked-to-shared") {
    return {
      label: deployment.is_symlink ? "symlink" : "linked folder",
      target: deploymentLinkTarget(deployment),
      broken: false,
    };
  }
  if (kind === "own" && deployment.is_symlink) {
    return { label: "symlink", target: deploymentLinkTarget(deployment), broken: false };
  }
  return null;
}

/** Bordered chip for a deployment's link relation, with the full target path as its tooltip. */
function LinkChip({ deployment }: { deployment: Deployment }) {
  const chip = linkChipFor(deployment);
  if (!chip) return null;
  return (
    <TooltipControl content={chip.target ? homeRelativePath(chip.target) : "unknown target"}>
      <span
        className={`inline-flex shrink-0 items-center gap-1 whitespace-nowrap rounded-sm border bg-bg-tertiary px-1.5 py-0.5 text-caption tracking-[0.02em] ${
          chip.broken ? "border-error text-error" : "border-border-subtle text-text-secondary"
        }`}
      >
        {chip.broken && <AlertTriangle size={11} />}
        <Link2 size={11} />
        {chip.label}
      </span>
    </TooltipControl>
  );
}

/** "Claude Code" / "Codex" / ... for a deployment's harness label, "Shared folder" for the shared root. */
function harnessDisplayName(deployment: Deployment): string {
  return deployment.agent === "shared" ? "Shared folder" : deployment.agent;
}

/** The last path segment, for a project row's muted prefix chip. */
function basename(path: string): string {
  return path.split("/").filter(Boolean).pop() ?? path;
}

/** Everything before the last path segment, for the skills root of a deployment's skill dir. */
function dirnameOf(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx > 0 ? path.slice(0, idx) : "/";
}

/**
 * Sorts deployments for display: global scope first, then project rows
 * grouped by project folder name (A→Z); within a group the shared root
 * first, then harnesses in `FIRST_CLASS_AGENTS` order.
 */
function sortedDeployments(deployments: Deployment[]): Deployment[] {
  function rank(d: Deployment): [number, string, number] {
    const projectGroup = d.scope === "project" && d.project_path ? basename(d.project_path) : "";
    // SAFETY: indexOf on a readonly string tuple needs its search value typed
    // as one of the tuple's members; -1 (not found) is a valid, handled
    // result for any other agent label, so a non-member string is harmless.
    const agentRank = d.agent === "shared" ? -1 : FIRST_CLASS_AGENTS.indexOf(d.agent as never);
    return [projectGroup ? 1 : 0, projectGroup, agentRank];
  }
  return [...deployments].sort((a, b) => {
    const [groupA, projectA, agentA] = rank(a);
    const [groupB, projectB, agentB] = rank(b);
    if (groupA !== groupB) return groupA - groupB;
    if (projectA !== projectB) return projectA.localeCompare(projectB);
    return agentA - agentB;
  });
}

function DeploymentRow({
  deployment,
  skill,
  showPath = true,
}: {
  deployment: Deployment;
  skill: InstalledSkill;
  /** Nested rows under the shared group skip the path - the group header already states it. */
  showPath?: boolean;
}) {
  const addToast = useAppStore((state) => state.addToast);
  const [isTogglingHarness, setIsTogglingHarness] = useState(false);
  const harnessId = harnessIdFromLabel(deployment.agent);
  const relation = relationLabel(deployment);
  const supportsNativeToggle = canToggleHarness(deployment);
  const supportsDisableSwitch =
    supportsNativeToggle || canMoveAsideDisable(deployment) || deployment.shared_via_whole_dir_link;

  const handleReveal = () => {
    openSkillPath(deployment.path, "reveal").catch((err) => {
      addToast({
        type: "error",
        title: "Couldn't reveal in Finder",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    });
  };

  const handleToggleEnabled = async (nextEnabled: boolean) => {
    // A harness whose skills root is itself a symlink to the whole shared
    // folder has no per-skill switch on disk - `set_deployment_enabled` and
    // `set_harness_enabled` both deterministically refuse it. Disabling
    // converts the whole-dir link to per-skill links first
    // (`explode_shared_dir`), so this is confirmed before it runs; enabling
    // has nothing destructive to confirm.
    if (deployment.shared_via_whole_dir_link) {
      if (!nextEnabled) {
        const confirmed = await ask(
          `${harnessDisplayName(deployment)} links the whole shared folder. Skill Studio will convert it to per-skill links first, then disable just ${skill.name}.`,
          { title: "Convert to per-skill links?", kind: "warning" },
        );
        if (!confirmed) return;
      }
      setIsTogglingHarness(true);
      try {
        await setSharedHarnessSkillEnabled(
          dirnameOf(deployment.path),
          basename(deployment.path),
          agentIdFromDeploymentLabel(deployment.agent) ?? "",
          nextEnabled,
        );
        setIsTogglingHarness(false);
      } catch (err) {
        addToast({
          type: "error",
          title: nextEnabled ? "Couldn't enable" : "Couldn't disable",
          message: err instanceof Error ? err.message : "Unknown error",
        });
        setIsTogglingHarness(false);
      }
      return;
    }

    setIsTogglingHarness(true);
    try {
      // A deployment already move-aside disabled, or one with no native
      // switch at all, always routes through the move-aside mechanism -
      // even if `supportsNativeToggle` happens to be true (e.g. a global
      // Claude Code symlink also qualifies for move-aside, but it's already
      // disabled the native way, so re-enabling must use the same path).
      if (deployment.disabled_by === "studio-moved" || !supportsNativeToggle) {
        await setDeploymentEnabled(skill.name, deployment.path, nextEnabled);
      } else {
        await setHarnessEnabled(
          skill.name,
          agentIdFromDeploymentLabel(deployment.agent) ?? "",
          nextEnabled,
        );
      }
      setIsTogglingHarness(false);
    } catch (err) {
      addToast({
        type: "error",
        title: nextEnabled ? "Couldn't enable" : "Couldn't disable",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      setIsTogglingHarness(false);
    }
  };

  return (
    <div
      className={`grid min-h-11 items-center gap-3 border-b border-border-subtle px-2 py-1.5 last:border-b-0 ${
        showPath ? "grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto]" : "grid-cols-[minmax(0,1fr)_auto]"
      }`}
    >
      <div className="flex min-w-0 items-center gap-2 text-body text-text-primary">
        {harnessId && <HarnessIcon harness={harnessId} size={16} />}
        <span className="shrink-0">{harnessDisplayName(deployment)}</span>
        {relation && (
          <span
            className="inline-block min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-small text-text-tertiary"
            title={relation}
          >
            {relation}
          </span>
        )}
        <LinkChip deployment={deployment} />
        {deployment.plugin && (
          <span className="inline-block min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-small text-text-tertiary">
            plugin · {deployment.plugin.name}
            {deployment.plugin.version ? ` v${deployment.plugin.version}` : ""}
          </span>
        )}
      </div>
      {showPath && (
        <TooltipControl content={deployment.path}>
          <span
            dir="rtl"
            className="overflow-hidden text-left text-ellipsis whitespace-nowrap font-mono text-small text-text-tertiary"
          >
            <span dir="ltr" className="[unicode-bidi:isolate]">
              {homeRelativePath(deployment.path)}
            </span>
          </span>
        </TooltipControl>
      )}
      <div className="flex shrink-0 items-center gap-3">
        {supportsDisableSwitch && (
          <label className="flex h-(--control-height) cursor-pointer items-center gap-2 text-small text-text-secondary">
            <SwitchControl
              checked={!deployment.disabled}
              onCheckedChange={handleToggleEnabled}
              disabled={isTogglingHarness}
              ariaLabel={`Enabled for ${deployment.agent}`}
            />
            Enabled
          </label>
        )}
        <TooltipControl content={`Reveal ${deployment.agent} copy in Finder`}>
          <button
            type="button"
            className="flex h-7 w-7 cursor-pointer items-center justify-center rounded-sm border-0 bg-transparent text-text-tertiary transition-colors hover:bg-bg-tertiary hover:text-text-primary"
            onClick={handleReveal}
            aria-label={`Reveal ${deployment.agent} copy in Finder`}
          >
            <FolderOpen size={14} />
          </button>
        </TooltipControl>
      </div>
    </div>
  );
}

/** Display label per native shared-root reader - `HARNESS_LABELS` doesn't cover cursor/grok-build. */
const READER_LABELS = [
  ["codex", "Codex"],
  ["open-code", "OpenCode"],
  ["pi", "pi"],
  ["cursor", "Cursor"],
  ["grok-build", "Grok Build"],
] satisfies [AgentId, string][];

/** A harness that reads the shared folder natively - no symlink to show. Codex and OpenCode have native per-skill disable mechanisms; pi, Cursor, and Grok Build read the folder unconditionally. */
function SharedReaderRow({
  agentId,
  skill,
  shared,
}: {
  agentId: AgentId;
  skill: InstalledSkill;
  shared: Deployment;
}) {
  const addToast = useAppStore((state) => state.addToast);
  const [isToggling, setIsToggling] = useState(false);
  const label = READER_LABELS.find(([id]) => id === agentId)?.[1] ?? agentId;
  const canToggle = agentId === "codex" || agentId === "open-code";
  const disabled = (shared.disabled_readers ?? []).includes(agentId);

  const handleToggle = async (nextEnabled: boolean) => {
    setIsToggling(true);
    try {
      await setHarnessEnabled(skill.name, agentId, nextEnabled);
      setIsToggling(false);
    } catch (err) {
      addToast({
        type: "error",
        title: nextEnabled ? "Couldn't enable" : "Couldn't disable",
        message: err instanceof Error ? err.message : "Unknown error",
      });
      setIsToggling(false);
    }
  };

  return (
    <div className="grid min-h-11 grid-cols-[minmax(0,1fr)_auto] items-center gap-3 border-b border-border-subtle px-2 py-1.5 last:border-b-0">
      <div className="flex min-w-0 items-center gap-2 text-body text-text-primary">
        <HarnessIcon harness={agentId} size={16} />
        <span className="shrink-0">{label}</span>
        <span className="inline-block min-w-0 overflow-hidden text-ellipsis whitespace-nowrap text-small text-text-tertiary">
          reads this folder
        </span>
      </div>
      {canToggle ? (
        <label className="flex h-(--control-height) cursor-pointer items-center gap-2 text-small text-text-secondary">
          <SwitchControl
            checked={!disabled}
            onCheckedChange={handleToggle}
            disabled={isToggling}
            ariaLabel={`Enabled for ${label}`}
          />
          Enabled
        </label>
      ) : (
        <TooltipControl
          content={`${label} reads the shared folder directly - it has no per-skill off switch`}
        >
          <span className="text-small text-text-tertiary">always on</span>
        </TooltipControl>
      )}
    </div>
  );
}

/**
 * The shared folder's own row, expandable to the harnesses linked to it -
 * "click the shared folder to see who links to it, click one of those to
 * disable it there" from the user's feedback.
 */
function SharedDeploymentGroup({
  group,
  skill,
}: {
  group: DeploymentGroup;
  skill: InstalledSkill;
}) {
  const [expanded, setExpanded] = useState(true);
  const addToast = useAppStore((state) => state.addToast);
  const readers = AGENTS_READING_SHARED_ROOT.filter(
    (id) => !group.linked.some((d) => agentIdFromDeploymentLabel(d.agent) === id),
  );
  const harnessCount = group.linked.length + readers.length;

  const handleDistribute = async () => {
    const confirmed = await ask(
      `Move "${skill.name}" out of the shared folder?\n\nEach harness that reads the shared folder gets its own copy in its own skills folder, so you can turn the skill off per harness. Harnesses that already have their own copy keep it.\n\nYou can undo this later from Activity → History.`,
      { title: "Move out of shared folder", kind: "info" },
    );
    if (!confirmed) return;
    try {
      await distributeSkillFromShared(dirnameOf(group.shared.path), skill.name);
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't move out of shared folder",
        message: err instanceof Error ? err.message : "Unknown error",
      });
    }
  };

  const toggleExpanded = () => setExpanded((open) => !open);
  const headerContent = (
    <>
      <span className="flex min-w-0 items-center gap-2 text-body text-text-primary">
        <ChevronRight
          size={14}
          className={`shrink-0 text-text-tertiary transition-transform ${expanded ? "rotate-90" : ""}`}
        />
        <FolderOpen size={16} className="shrink-0" />
        <span className="shrink-0">Shared folder</span>
      </span>
      <TooltipControl content={group.shared.path}>
        <span
          dir="rtl"
          className="overflow-hidden text-left text-ellipsis whitespace-nowrap font-mono text-small text-text-tertiary"
        >
          <span dir="ltr" className="[unicode-bidi:isolate]">
            {homeRelativePath(group.shared.path)}
          </span>
        </span>
      </TooltipControl>
      <span className="flex shrink-0 items-center gap-3 justify-self-end">
        <span className="text-small tabular-nums text-text-tertiary">
          {`used by ${harnessCount} harness${harnessCount === 1 ? "" : "es"}`}
        </span>
        <button
          type="button"
          className="cursor-pointer border-0 bg-none text-small text-accent hover:underline"
          onClick={(e) => {
            e.stopPropagation();
            void handleDistribute();
          }}
        >
          Move out of shared…
        </button>
      </span>
    </>
  );
  const gridClass =
    "grid min-h-11 w-full grid-cols-[minmax(0,1fr)_minmax(0,1fr)_auto] items-center gap-3 px-2 py-1.5";
  return (
    <div className="border-b border-border-subtle last:border-b-0">
      <div
        className={`${gridClass} cursor-pointer rounded-sm text-left transition-colors hover:bg-bg-hover`}
        role="button"
        tabIndex={0}
        aria-expanded={expanded}
        onClick={toggleExpanded}
        onKeyDown={(e) => {
          if (e.key === "Enter" || e.key === " ") {
            e.preventDefault();
            toggleExpanded();
          }
        }}
      >
        {headerContent}
      </div>
      {expanded && (
        <div className="flex flex-col pl-[22px]">
          {group.linked.map((deployment) => (
            <DeploymentRow
              key={deployment.path}
              deployment={deployment}
              skill={skill}
              showPath={false}
            />
          ))}
          {readers.map((agentId) => (
            <SharedReaderRow key={agentId} agentId={agentId} skill={skill} shared={group.shared} />
          ))}
        </div>
      )}
    </div>
  );
}

type LocationItem =
  | { kind: "group"; group: DeploymentGroup }
  | { kind: "row"; deployment: Deployment };

/**
 * Buckets shared groups and standalone rows by project (global first, then
 * projects A→Z), so a project's name renders once as a section label
 * instead of being prefixed onto every one of its rows.
 */
function locationSections(
  groups: DeploymentGroup[],
  standalone: Deployment[],
): { label: string | null; items: LocationItem[] }[] {
  const sections = new Map<string | null, LocationItem[]>();
  const push = (key: string | null, item: LocationItem) => {
    const list = sections.get(key) ?? [];
    list.push(item);
    sections.set(key, list);
  };
  const keyOf = (d: Deployment) =>
    d.scope === "project" && d.project_path ? basename(d.project_path) : null;
  for (const group of groups) push(keyOf(group.shared), { kind: "group", group });
  for (const deployment of standalone) push(keyOf(deployment), { kind: "row", deployment });
  return [...sections.entries()]
    .sort(([a], [b]) => (a === null ? -1 : b === null ? 1 : a.localeCompare(b)))
    .map(([label, items]) => ({ label, items }));
}

/**
 * "Where it lives": the shared folder (when the skill has one) grouped with
 * the harnesses linked to it, every other deployment as its own row, plus an
 * Invocation footer row (segmented Both/User only/Model only control, a
 * one-line explanation, and an `allowed-tools` chip when the frontmatter sets
 * one).
 */
export function SkillLocationsCard({
  skill,
  skillMdPath,
  skillMdDeployment,
  onCompareCopies,
}: SkillLocationsCardProps) {
  const addToast = useAppStore((state) => state.addToast);
  const [isSavingInvocation, setIsSavingInvocation] = useState(false);
  const allowedTools = skill.frontmatter_fields["allowed-tools"];

  // A dotagents/skills.sh-managed skill would have its edits overwritten by
  // the next sync/update - forking first (same rule as the SKILL.md editor)
  // makes the invocation-policy change stick.
  const needsForkToSave = skill.source_kind === "dotagents" || skill.source_kind === "skills-sh";

  // A Promise chain, not a try/finally statement, so the compiler can still
  // optimize this component (it doesn't support `finally` clauses yet).
  const handleSetInvocation = (policy: InvocationPolicy) => {
    if (!skillMdPath || isSavingInvocation) return;
    setIsSavingInvocation(true);
    const forkIfNeeded =
      needsForkToSave && skillMdDeployment
        ? forkSkill(skill.name, skillMdDeployment.path)
        : Promise.resolve();
    return forkIfNeeded
      .then(() => setSkillInvocation(skill.name, skillMdPath, policy))
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't change invocation policy",
          message: err instanceof Error ? err.message : "Unknown error",
        });
      })
      .finally(() => {
        setIsSavingInvocation(false);
      });
  };

  const hasDriftingCopies = driftingCopies(locationSummary(skill)).length > 0;
  const { groups, standalone } = groupDeploymentsForDisplay(sortedDeployments(skill.deployments));
  const sections = locationSections(groups, standalone);

  return (
    <div className="flex flex-col gap-1 rounded-lg border border-border-subtle p-4">
      <div className="flex items-baseline justify-between gap-3 text-body font-semibold text-text-primary">
        Locations
        {hasDriftingCopies && onCompareCopies && (
          <button
            type="button"
            className="cursor-pointer border-0 bg-transparent p-0 text-small font-normal text-accent transition-colors hover:underline"
            onClick={onCompareCopies}
          >
            Compare copies
          </button>
        )}
      </div>

      {skill.deployments.length === 0 ? (
        <p className="m-0 px-3 py-6 text-small text-text-tertiary">
          Known only from the lock file — no folder on disk.
        </p>
      ) : (
        <div className="-mx-2 flex flex-col">
          {sections.map((section) => (
            <Fragment key={section.label ?? "global"}>
              {(section.label !== null || sections.length > 1) && (
                <div className="px-2 pt-3 pb-1 text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase first:pt-1">
                  {section.label ?? "Global"}
                </div>
              )}
              {section.items.map((item) =>
                item.kind === "group" ? (
                  <SharedDeploymentGroup
                    key={item.group.shared.path}
                    group={item.group}
                    skill={skill}
                  />
                ) : (
                  <DeploymentRow
                    key={item.deployment.path}
                    deployment={item.deployment}
                    skill={skill}
                  />
                ),
              )}
            </Fragment>
          ))}
        </div>
      )}

      <div className="mt-3 flex flex-col gap-1.5 border-t border-border-subtle pt-3">
        <span className="text-caption font-medium tracking-[0.08em] text-text-tertiary uppercase">
          Invocation
        </span>
        {skillMdPath && (
          <div className="flex" role="group" aria-label="Invocation">
            {INVOCATION_POLICY_OPTIONS.map((option) => (
              <button
                key={option.value}
                type="button"
                className="inline-flex h-(--control-height) cursor-pointer items-center gap-1 border border-l-0 border-border px-3 text-body text-text-tertiary transition-colors first:rounded-l-sm first:border-l last:rounded-r-sm hover:bg-bg-hover hover:text-text-secondary aria-pressed:bg-bg-tertiary aria-pressed:text-text-primary"
                aria-pressed={skill.invocation === option.value}
                onClick={() => handleSetInvocation(option.value)}
                disabled={isSavingInvocation || skill.invocation === option.value}
              >
                {option.label}
              </button>
            ))}
          </div>
        )}
        <p className="text-small text-text-tertiary">
          {invocationPolicyExplanation(skill.invocation, skill.name)}
        </p>
        {allowedTools && (
          <div className="text-small text-text-tertiary">Allowed tools: {allowedTools}</div>
        )}
      </div>
    </div>
  );
}
