// ============================================================================
// Skill Studio - skill-list-filter
// Pure filter over a skill list: scope (all/global/parked/project), one
// harness, one source kind, one issue kind, and a free-text query. Shared by
// SkillListFilterBar and SkillsView so the filter bar's controls and the
// list they drive never drift apart.
// ============================================================================

import type { HealthIssueKind } from "./skill-health";
import { collectDashboardIssues } from "./skill-health";
import { pluginDeployments } from "./skill-plugin-partition";
import type { InstalledSkill, SkillSourceKind } from "./skill-types";

/** Which own skills `applySkillListFilter` considers before the other fields narrow it further. */
export type SkillListFilterScope = "all" | "global" | "parked" | { project: string };

/** The Skills view's filter bar state: scope, an optional harness/source/issue narrower, and a query. */
export interface SkillListFilter {
  scope: SkillListFilterScope;
  /** An `AgentId` display label (e.g. "Claude Code"), one at a time. */
  harness?: string;
  source?: SkillSourceKind;
  /** `"any"` keeps every skill with at least one issue, regardless of kind - Home's "Show all N". */
  issue?: HealthIssueKind | "any";
  query: string;
}

/** A filter with every field at its default: every non-parked skill, no query. */
export function defaultSkillListFilter(): SkillListFilter {
  return { scope: "all", query: "" };
}

/** True when `scope` is the `{ project: string }` variant, narrowing its type for callers. */
export function isProjectScope(scope: SkillListFilterScope): scope is { project: string } {
  return scope !== "all" && scope !== "global" && scope !== "parked";
}

/** Whether `skill` belongs to `scope`. `all` and `global` never include parked skills. */
function matchesScope(skill: InstalledSkill, scope: SkillListFilterScope): boolean {
  if (scope === "parked") return skill.parked;
  if (skill.parked) return false;
  if (scope === "all") return true;
  if (scope === "global") {
    return skill.deployments.some((d) => d.scope === "global" || d.scope === "plugin");
  }
  return skill.deployments.some((d) => d.project_path === scope.project);
}

/** Whether `skill` has a deployment for the harness display label `harness`. */
function matchesHarness(skill: InstalledSkill, harness: string): boolean {
  return skill.deployments.some((d) => d.agent === harness);
}

/** Whether `skill` matches `query` in its name, description, or source repo, case-insensitive. */
function matchesQuery(skill: InstalledSkill, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return (
    skill.name.toLowerCase().includes(q) ||
    (skill.description ?? "").toLowerCase().includes(q) ||
    skill.source.toLowerCase().includes(q)
  );
}

/**
 * Filters `skills` by every field of `filter` in turn: scope, then harness,
 * source kind, and issue kind (each only when set), then the free-text
 * query. `issues` should be `collectDashboardIssues(skills)` (or equivalent)
 * from the caller, computed once and shared - passed in rather than
 * recomputed here so a caller filtering a large list repeatedly doesn't pay
 * for it more than once per render.
 */
export function applySkillListFilter(
  skills: InstalledSkill[],
  filter: SkillListFilter,
  issues = collectDashboardIssues(skills),
): InstalledSkill[] {
  const skillsWithIssue = filter.issue
    ? new Set(
        issues
          .filter((i) => filter.issue === "any" || i.kind === filter.issue)
          .map((i) => i.skill.name),
      )
    : null;

  return skills.filter((skill) => {
    if (!matchesScope(skill, filter.scope)) return false;
    if (filter.harness && !matchesHarness(skill, filter.harness)) return false;
    // "plugin" reaches outside a skill's own source_kind: it means "has at
    // least one plugin deployment", tested against whatever deployments the
    // caller passed in (the pluginSkillsView) rather than the aggregate
    // source_kind, which describes the skill's own (non-plugin) copies.
    if (filter.source === "plugin") {
      if (pluginDeployments(skill).length === 0) return false;
    } else if (filter.source && skill.source_kind !== filter.source) {
      return false;
    }
    if (skillsWithIssue && !skillsWithIssue.has(skill.name)) return false;
    if (!matchesQuery(skill, filter.query)) return false;
    return true;
  });
}
