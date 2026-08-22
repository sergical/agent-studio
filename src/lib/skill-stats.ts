// ============================================================================
// Agent Studio - skill-stats
// Pure aggregation helpers over a SkillSnapshot: dashboard totals, top
// skills by recent invocations, and the skill x agent deployment matrix.
// ============================================================================

import type { InstalledSkill, SkillInvocationStats, SkillSnapshot } from "./skill-types";

/** Aggregate counts shown in the dashboard's stat cards. */
export interface SkillTotals {
  skillCount: number;
  tokens: number;
  bytes: number;
  invocationsLast30Days: number;
}

/**
 * Totals over every skill in the snapshot: count, SKILL.md token sum,
 * folder byte sum, and invocations across all skills in the last 30 days.
 */
export function computeTotals(snapshot: SkillSnapshot): SkillTotals {
  let tokens = 0;
  let bytes = 0;
  for (const skill of snapshot.skills) {
    tokens += skill.skill_md_tokens;
    bytes += skill.folder_bytes;
  }

  let invocationsLast30Days = 0;
  for (const stat of snapshot.invocations) {
    invocationsLast30Days += stat.last_30_days;
  }

  return { skillCount: snapshot.skills.length, tokens, bytes, invocationsLast30Days };
}

/** Formats a byte count as e.g. "1.2 MB", "340 KB", "12 B". */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(1)} ${units[unit]}`;
}

/** Formats a token count as e.g. "12.3k" above 1,000, otherwise the raw number. */
export function formatTokens(tokens: number): string {
  if (tokens < 1000) return tokens.toString();
  return `${(tokens / 1000).toFixed(1)}k`;
}

/** The `n` skills with the most invocations in the last 30 days, descending. */
export function topSkills(stats: SkillInvocationStats[], n: number): SkillInvocationStats[] {
  return [...stats].sort((a, b) => b.last_30_days - a.last_30_days).slice(0, n);
}

/** Column order for `agentMatrix`, shared by the dashboard and detail panel. */
export const AGENT_MATRIX_LABELS = ["Claude Code", "Codex", "OpenCode", "pi", "shared"] as const;

export type AgentMatrixLabel = (typeof AGENT_MATRIX_LABELS)[number];

/** Deployment coverage for one agent column in a matrix row. */
export type AgentMatrixCell = "global" | "project" | "both" | null;

/** One row of `agentMatrix`: a skill and its coverage per agent. */
export interface AgentMatrixRow {
  skill: InstalledSkill;
  cells: Record<AgentMatrixLabel, AgentMatrixCell>;
}

/** Narrows a deployment's agent name to a known matrix column, if it is one. */
function isAgentMatrixLabel(agent: string): agent is AgentMatrixLabel {
  return AGENT_MATRIX_LABELS.some((label) => label === agent);
}

/** A matrix row's cells, all starting undeployed. */
function emptyMatrixCells(): Record<AgentMatrixLabel, AgentMatrixCell> {
  // SAFETY: mapping every AGENT_MATRIX_LABELS entry to `null` produces
  // exactly the keys of Record<AgentMatrixLabel, AgentMatrixCell>.
  return Object.fromEntries(AGENT_MATRIX_LABELS.map((label) => [label, null])) as Record<
    AgentMatrixLabel,
    AgentMatrixCell
  >;
}

/**
 * Builds the skill x agent deployment matrix: for every skill, which agents
 * it's deployed to and whether that's at global scope, project scope, or
 * both. Rows follow `skills`' input order; sort before calling if needed.
 */
export function agentMatrix(skills: InstalledSkill[]): AgentMatrixRow[] {
  return skills.map((skill) => {
    const cells = emptyMatrixCells();

    for (const deployment of skill.deployments) {
      if (!isAgentMatrixLabel(deployment.agent)) continue;
      const label = deployment.agent;

      const isGlobal = deployment.scope === "global" || deployment.scope === "plugin";
      const existing = cells[label];
      if (existing === null) {
        cells[label] = isGlobal ? "global" : "project";
      } else if (existing !== "both") {
        const newValue = isGlobal ? "global" : "project";
        cells[label] = existing === newValue ? existing : "both";
      }
    }

    return { skill, cells };
  });
}
