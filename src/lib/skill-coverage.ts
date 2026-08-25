// ============================================================================
// Skill Studio - skill-coverage
// Effective visibility of a skill to an agent: the shared `.agents/skills`
// root is read natively by Codex, OpenCode and pi, so a skill deployed only
// there is still visible to them even without a deployment in their own
// directory. Claude Code does not read the shared root (see
// docs/agent-skill-conventions.md line 67); it needs its own deployment.
// ============================================================================

import { ownDeployments } from "./skill-plugin-partition";
import type { AgentId, Deployment, InstalledSkill } from "./skill-types";

/** The three first-class agents that read the shared `.agents/skills` root natively. */
export const AGENTS_READING_SHARED_ROOT: readonly AgentId[] = ["codex", "open-code", "pi"];

/** The `Deployment.agent` value for a deployment placed in the shared root, not any one agent's own folder. */
const SHARED_AGENT_ID = "shared";

/**
 * Maps a `Deployment.agent` display label (the Rust side serializes
 * `AgentId::display_name()`, e.g. "Claude Code") to the first-class agent id.
 * `null` for the shared root and for agents outside the first-class set.
 */
export function agentIdFromDeploymentLabel(label: string): AgentId | "shared" | null {
  switch (label) {
    case "Claude Code":
      return "claude-code";
    case "Codex":
      return "codex";
    case "OpenCode":
      return "open-code";
    case "pi":
      return "pi";
    case SHARED_AGENT_ID:
      return "shared";
    default:
      return null;
  }
}

/**
 * True when a deployment's symlink doesn't resolve: either the target is
 * confirmed missing (`symlink_is_broken`), or resolving it failed for some
 * other reason (`symlink_error`, e.g. a permissions error or a symlink loop).
 * Either way the deployment can't back visibility.
 */
export function isUnresolvedDeployment(deployment: Deployment): boolean {
  return deployment.symlink_is_broken || deployment.symlink_error != null;
}

/** A deployment counts toward visibility only when its symlink (if any) resolves. */
function isOwnDirDeployment(skill: InstalledSkill, agent: AgentId): boolean {
  return ownDeployments(skill).some(
    (d) =>
      agentIdFromDeploymentLabel(d.agent) === agent &&
      (d.scope === "global" || d.scope === "project") &&
      !isUnresolvedDeployment(d),
  );
}

/** True when the deployment is a symlink whose target lives in a `.agents/skills` root. */
function isLinkedToSharedRoot(target: string | undefined): boolean {
  return target !== undefined && /\/\.agents\/skills\//.test(target + "/");
}

/**
 * Classifies a single deployment for the small link marker shown next to its
 * harness chip: `shared-root` when it *is* the shared `.agents/skills` copy,
 * `linked-to-shared` when it's a symlink pointing into that shared root,
 * `broken` when its symlink target doesn't resolve, else `own`.
 */
export function deploymentLinkKind(
  deployment: Deployment,
): "shared-root" | "linked-to-shared" | "own" | "broken" {
  if (isUnresolvedDeployment(deployment)) return "broken";
  if (deployment.agent === SHARED_AGENT_ID) return "shared-root";
  if (deployment.is_symlink && isLinkedToSharedRoot(deployment.symlink_target)) {
    return "linked-to-shared";
  }
  return "own";
}

/** A broken shared-root deployment doesn't make a skill visible via the shared root. */
function isSharedRootDeployment(skill: InstalledSkill): boolean {
  return ownDeployments(skill).some(
    (d) => d.agent === SHARED_AGENT_ID && !isUnresolvedDeployment(d),
  );
}

/**
 * Whether `agent` can actually see `skill`: "own" via a deployment in the
 * agent's own directory, "shared" via the shared root (only for agents that
 * read it), or "none". This is effective visibility only - it says nothing
 * about whether the agent's *own* deployment is a healthy link; a broken own
 * deployment with a healthy shared fallback still reports "shared" here.
 * `cellForAgent` layers that local-link health back on for the matrix.
 */
export function skillVisibleToAgent(
  skill: InstalledSkill,
  agent: AgentId,
): "own" | "shared" | "none" {
  if (isOwnDirDeployment(skill, agent)) return "own";
  if (AGENTS_READING_SHARED_ROOT.includes(agent) && isSharedRootDeployment(skill)) return "shared";
  return "none";
}

/** Coverage totals for the dashboard's two-row table: Claude Code's own folder vs. the shared root. */
export interface CoverageSummary {
  claudeCode: {
    visible: number;
    missing: number;
    /** Of the visible skills, how many are symlinks into a shared `.agents/skills` root. */
    linkedToShared: number;
  };
  shared: {
    visible: number;
    missing: number;
    /** Skills not in the shared root but present in that agent's own directory, keyed by agent (zeros omitted). */
    onlyInOwnDir: Partial<Record<AgentId, number>>;
  };
  total: number;
}

/** Summarizes effective visibility over `skills` (own skills only - see `ownSkillsView`). */
export function summarizeCoverage(skills: InstalledSkill[]): CoverageSummary {
  const total = skills.length;
  let claudeVisible = 0;
  let claudeLinkedToShared = 0;
  let sharedVisible = 0;
  const onlyInOwnDir: Partial<Record<AgentId, number>> = {};

  for (const skill of skills) {
    if (skillVisibleToAgent(skill, "claude-code") !== "none") {
      claudeVisible += 1;
      const linked = ownDeployments(skill).some(
        (d) =>
          agentIdFromDeploymentLabel(d.agent) === "claude-code" &&
          d.is_symlink &&
          isLinkedToSharedRoot(d.symlink_target),
      );
      if (linked) claudeLinkedToShared += 1;
    }

    const inSharedRoot = isSharedRootDeployment(skill);
    if (inSharedRoot) {
      sharedVisible += 1;
      continue;
    }

    for (const agent of AGENTS_READING_SHARED_ROOT) {
      if (isOwnDirDeployment(skill, agent)) {
        onlyInOwnDir[agent] = (onlyInOwnDir[agent] ?? 0) + 1;
      }
    }
  }

  return {
    claudeCode: {
      visible: claudeVisible,
      missing: total - claudeVisible,
      linkedToShared: claudeLinkedToShared,
    },
    shared: { visible: sharedVisible, missing: total - sharedVisible, onlyInOwnDir },
    total,
  };
}

// ============================================================================
// Skill x agent matrix (moved from skill-stats.ts): now reflects effective
// visibility rather than raw per-agent deployments.
// ============================================================================

/** Column order for the per-agent columns of the coverage matrix. */
export const AGENT_MATRIX_LABELS = ["Claude Code", "Codex", "OpenCode", "pi"] as const;

export type AgentMatrixLabel = (typeof AGENT_MATRIX_LABELS)[number];

/** The `AgentId` each matrix column label corresponds to. */
const AGENT_MATRIX_AGENT_IDS = {
  "Claude Code": "claude-code",
  Codex: "codex",
  OpenCode: "open-code",
  pi: "pi",
} satisfies Record<AgentMatrixLabel, AgentId>;

/**
 * One matrix cell: whether the skill is visible (`state`), and whether the
 * link backing that visibility is a (broken) symlink. `isBroken` also flags a
 * broken own-directory deployment even when `state` is "shared" because a
 * healthy shared copy still makes the skill visible - the marker exists so
 * the matrix keeps showing the local breakage worth fixing.
 */
export interface AgentMatrixCell {
  state: "own" | "shared" | "none";
  isSymlink: boolean;
  isBroken: boolean;
}

/** One row of `agentMatrix`: a skill, its shared-root cell, and its per-agent cells. */
export interface AgentMatrixRow {
  skill: InstalledSkill;
  shared: AgentMatrixCell;
  cells: Record<AgentMatrixLabel, AgentMatrixCell>;
}

const EMPTY_CELL: AgentMatrixCell = { state: "none", isSymlink: false, isBroken: false };

function cellForAgent(skill: InstalledSkill, agent: AgentId): AgentMatrixCell {
  const state = skillVisibleToAgent(skill, agent);
  const own = ownDeployments(skill);
  const ownDeployment = own.find(
    (d) =>
      agentIdFromDeploymentLabel(d.agent) === agent &&
      (d.scope === "global" || d.scope === "project"),
  );

  if (state === "none") {
    // The own-dir copy is broken and there's no healthy shared fallback: the
    // skill is effectively invisible to this agent, but keep the broken
    // marker in the matrix rather than reporting a plain empty cell.
    if (ownDeployment && isUnresolvedDeployment(ownDeployment)) {
      return { state: "none", isSymlink: ownDeployment.is_symlink, isBroken: true };
    }
    return EMPTY_CELL;
  }

  const deployment = state === "own" ? ownDeployment : own.find((d) => d.agent === SHARED_AGENT_ID);

  return {
    state,
    isSymlink: deployment?.is_symlink ?? false,
    // A healthy shared fallback (`state === "shared"`) doesn't hide a broken
    // own-directory deployment - the matrix still marks it broken so it's
    // visible as something worth fixing, even though the skill remains
    // effectively visible to the agent via the shared root.
    isBroken:
      (deployment !== undefined && isUnresolvedDeployment(deployment)) ||
      (ownDeployment !== undefined && isUnresolvedDeployment(ownDeployment)),
  };
}

function cellForSharedRoot(skill: InstalledSkill): AgentMatrixCell {
  const deployment = ownDeployments(skill).find((d) => d.agent === SHARED_AGENT_ID);
  if (!deployment) return EMPTY_CELL;
  return { state: "own", isSymlink: deployment.is_symlink, isBroken: deployment.symlink_is_broken };
}

/**
 * Builds the skill x agent visibility matrix: for every skill, whether each
 * first-class agent sees it (own directory or via the shared root), plus a
 * dedicated cell for the shared root itself. Rows follow `skills`' input
 * order; sort before calling if needed.
 */
export function agentMatrix(skills: InstalledSkill[]): AgentMatrixRow[] {
  return skills.map((skill) => ({
    skill,
    shared: cellForSharedRoot(skill),
    // SAFETY: mapping every AGENT_MATRIX_LABELS entry produces exactly the
    // keys of Record<AgentMatrixLabel, AgentMatrixCell>.
    cells: Object.fromEntries(
      AGENT_MATRIX_LABELS.map((label) => [
        label,
        cellForAgent(skill, AGENT_MATRIX_AGENT_IDS[label]),
      ]),
    ) as Record<AgentMatrixLabel, AgentMatrixCell>,
  }));
}
