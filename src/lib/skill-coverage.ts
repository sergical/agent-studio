// ============================================================================
// Skill Studio - skill-coverage
// Effective visibility of a skill to an agent: the shared `.agents/skills`
// root is read natively by Codex, OpenCode and pi, so a skill deployed only
// there is still visible to them even without a deployment in their own
// directory. Claude Code does not read the shared root (see
// docs/agent-skill-conventions.md line 67); it needs its own deployment.
// ============================================================================

import { ownDeployments } from "./skill-plugin-partition";
import type { AgentId, InstalledSkill } from "./skill-types";

/** The three first-class agents that read the shared `.agents/skills` root natively. */
export const AGENTS_READING_SHARED_ROOT: readonly AgentId[] = ["codex", "open-code", "pi"];

/** The `Deployment.agent` value for a deployment placed in the shared root, not any one agent's own folder. */
const SHARED_AGENT_ID = "shared";

function isOwnDirDeployment(skill: InstalledSkill, agent: AgentId): boolean {
  return ownDeployments(skill).some(
    (d) => d.agent === agent && (d.scope === "global" || d.scope === "project"),
  );
}

function isSharedRootDeployment(skill: InstalledSkill): boolean {
  return ownDeployments(skill).some((d) => d.agent === SHARED_AGENT_ID);
}

/**
 * Whether `agent` can actually see `skill`: "own" via a deployment in the
 * agent's own directory, "shared" via the shared root (only for agents that
 * read it), or "none".
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
  claudeCode: { visible: number; missing: number };
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
  let sharedVisible = 0;
  const onlyInOwnDir: Partial<Record<AgentId, number>> = {};

  for (const skill of skills) {
    if (skillVisibleToAgent(skill, "claude-code") !== "none") claudeVisible += 1;

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
    claudeCode: { visible: claudeVisible, missing: total - claudeVisible },
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

/** One matrix cell: whether the skill is visible, and whether that deployment is a (broken) symlink. */
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
  if (state === "none") return EMPTY_CELL;

  const own = ownDeployments(skill);
  const deployment =
    state === "own"
      ? own.find((d) => d.agent === agent && (d.scope === "global" || d.scope === "project"))
      : own.find((d) => d.agent === SHARED_AGENT_ID);

  return {
    state,
    isSymlink: deployment?.is_symlink ?? false,
    isBroken: deployment?.symlink_is_broken ?? false,
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
