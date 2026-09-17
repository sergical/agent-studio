// ============================================================================
// home-inbox-data - Derives Home's inbox groups (Broken, Warnings, Updates,
// Not used in 30 days, Recently used) from a scan snapshot, and lays out
// their rows into one continuous aria-rowindex/cursor-key sequence.
// ============================================================================

import {
  attentionGroups,
  collectDashboardIssues,
  homeInvocationCounts,
  homePromptCost,
  ownSkillsView,
  recentlyUsedSkills,
  skillsWithUpdates,
  unusedSkills,
} from "@skill-studio/lib";
import type {
  HealthIssue,
  HealthIssueKind,
  InstalledSkill,
  RecentlyUsedSkill,
  SkillSnapshot,
} from "@skill-studio/lib";

/** How many of "Recently used" to show. */
export const RECENTLY_USED_COUNT = 5;
/** How many rows any other group shows before it collapses into a "Show all" link. */
export const MAX_ROWS_PER_GROUP = 6;

/** The one filter that can be active at a time: a stat tile or the idle bar segment. */
export type HomeFilter = "broken" | "warn" | "upd" | "unused";

/** Every inbox group, in display order - also the key `HomeFilter` narrows to. */
export type GroupId = HomeFilter | "rec";

/** A row's `aria-rowindex` from its group's start offset and its position in that group - pure,
 * so groups needn't share a mutable counter. */
export function rowAt(start: number, i: number): number {
  return start + i + 1;
}

/** One row's key for `useRowCursor` - namespaced by group id, since a skill can appear in more
 * than one Home group (e.g. broken and unused) and each occurrence needs its own cursor stop. */
export function issueKey(groupId: GroupId, issue: HealthIssue): string {
  return `${groupId}:${issue.kind}:${issue.skill.name}:${issue.detail}`;
}
export function skillKey(groupId: GroupId, skill: InstalledSkill): string {
  return `${groupId}:${skill.name}`;
}

/** The row-level action label for one health issue kind - see NeedsAttentionCard's former mapping. */
export function issueActionLabel(kind: HealthIssueKind): string {
  switch (kind) {
    case "broken-symlink":
      return "Fix link";
    case "duplicate":
      return "Compare";
    case "linked-root":
      return "Convert to per-skill links";
    case "parked-but-reinstalled":
    case "spec-violation":
    case "lock-only":
      return "Open";
  }
}

export interface HomeGroups {
  own: InstalledSkill[];
  broken: HealthIssue[];
  warnings: HealthIssue[];
  updates: InstalledSkill[];
  inv: ReturnType<typeof homeInvocationCounts>;
  cost: ReturnType<typeof homePromptCost>;
  unused: InstalledSkill[];
  recent: RecentlyUsedSkill[];
  allClear: boolean;
}

/**
 * Derives every inbox group and lane-card figure from a scan snapshot - falls back to empty
 * arrays when there's no snapshot yet, since every Hook in `HomeView` must still run on that
 * render (its skeleton/empty states return only after they've all been called).
 */
export function computeHomeGroups(snapshot: SkillSnapshot | undefined): HomeGroups {
  const own = snapshot ? ownSkillsView(snapshot.skills) : [];
  const issues = collectDashboardIssues(own);
  const { broken, warnings } = attentionGroups(issues);
  const updates = snapshot ? skillsWithUpdates(snapshot) : [];
  const inv = homeInvocationCounts(own);
  const cost = homePromptCost(own, snapshot?.invocations ?? []);
  const unused = unusedSkills(own, snapshot?.invocations ?? []);
  const recent = snapshot
    ? recentlyUsedSkills(snapshot.skills, snapshot.invocations, RECENTLY_USED_COUNT)
    : [];
  const allClear = broken.length === 0 && warnings.length === 0 && updates.length === 0;
  return { own, broken, warnings, updates, inv, cost, unused, recent, allClear };
}

export interface HomeRowPlan {
  starts: { broken: number; warn: number; upd: number; unused: number; rec: number };
  visibleKeys: string[];
  openByKey: Map<string, () => void>;
}

/**
 * Lays out the visible-and-expanded rows of every group into one continuous `aria-rowindex`
 * sequence and one cursor key space - in the same visible-and-expanded order `HomeView` renders
 * them, so a collapsed or filtered-out group's rows drop out of both.
 */
export function buildHomeRowPlan(params: {
  groups: HomeGroups;
  isGroupVisible: (id: GroupId) => boolean;
  isGroupExpanded: (id: GroupId) => boolean;
  onSelectSkill: (name: string) => void;
}): HomeRowPlan {
  const { groups, isGroupVisible, isGroupExpanded, onSelectSkill } = params;
  const { broken, warnings, updates, unused, recent } = groups;

  // Each group's rendered count, capped at `MAX_ROWS_PER_GROUP` except "Recently used", which has
  // none - plain offsets rather than a mutable counter, so groups render independently of one
  // another.
  const groupCount = (id: GroupId, total: number, capped = true) =>
    isGroupVisible(id) ? (capped ? Math.min(total, MAX_ROWS_PER_GROUP) : total) : 0;
  const brokenStart = 0;
  const warnStart = brokenStart + groupCount("broken", broken.length);
  const updStart = warnStart + groupCount("warn", warnings.length);
  const unusedStart = updStart + groupCount("upd", updates.length);
  const recStart = unusedStart + groupCount("unused", unused.length);

  // The cursor's row keys and their open actions, in the same visible-and-expanded order the JSX
  // renders - a collapsed or filtered-out group's rows drop out of both.
  const rowsFor = <T>(id: GroupId, all: T[], capped = true): T[] =>
    isGroupVisible(id) && isGroupExpanded(id)
      ? capped
        ? all.slice(0, MAX_ROWS_PER_GROUP)
        : all
      : [];
  const brokenRows = rowsFor("broken", broken);
  const warnRows = rowsFor("warn", warnings);
  const updRows = rowsFor("upd", updates);
  const unusedRows = rowsFor("unused", unused);
  const recRows = rowsFor("rec", recent, false);

  const visibleKeys = [
    ...brokenRows.map((issue) => issueKey("broken", issue)),
    ...warnRows.map((issue) => issueKey("warn", issue)),
    ...updRows.map((skill) => skillKey("upd", skill)),
    ...unusedRows.map((skill) => skillKey("unused", skill)),
    ...recRows.map(({ skill }) => skillKey("rec", skill)),
  ];
  const openByKey = new Map<string, () => void>([
    ...brokenRows.map((issue): [string, () => void] => [
      issueKey("broken", issue),
      () => onSelectSkill(issue.skill.name),
    ]),
    ...warnRows.map((issue): [string, () => void] => [
      issueKey("warn", issue),
      () => onSelectSkill(issue.skill.name),
    ]),
    ...updRows.map((skill): [string, () => void] => [
      skillKey("upd", skill),
      () => onSelectSkill(skill.name),
    ]),
    ...unusedRows.map((skill): [string, () => void] => [
      skillKey("unused", skill),
      () => onSelectSkill(skill.name),
    ]),
    ...recRows.map(({ skill }): [string, () => void] => [
      skillKey("rec", skill),
      () => onSelectSkill(skill.name),
    ]),
  ]);

  return {
    starts: {
      broken: brokenStart,
      warn: warnStart,
      upd: updStart,
      unused: unusedStart,
      rec: recStart,
    },
    visibleKeys,
    openByKey,
  };
}
