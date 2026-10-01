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
  LifecycleTarget,
  PullResult,
  RecentlyUsedSkill,
  SkillSnapshot,
  UpdateAllOutcome,
} from "@skill-studio/lib";
import { lifecycleTargetForPark, skillUpdateOwnerTargets } from "../../lib/skill-lifecycle-target";
import { issueRowState, rowState, updateRowState } from "../SkillList/skill-row-state";
import type { RowState } from "../SkillList/skill-row-state";

/** How many of "Recently used" to show. */
const RECENTLY_USED_COUNT = 5;
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

/** The state a row shows, by which group it sits in - so a Broken/Warnings row always matches the
 * group's own severity instead of `rowState`'s ladder over the skill's other conditions (an
 * unrelated update, or nothing at all for a warning kind the ladder doesn't know about). `issue`
 * is required for "broken"/"warn" (every row in those groups has one) and ignored elsewhere. */
export function homeRowState(
  group: GroupId,
  skill: InstalledSkill,
  issue: HealthIssue | null,
): RowState | null {
  switch (group) {
    case "broken":
    case "warn":
      return issue ? issueRowState(issue) : null;
    case "upd":
      return updateRowState(skill);
    case "unused":
    case "rec":
      return rowState(skill);
  }
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

interface UpdateAllTally {
  attempted: number;
  succeeded: number;
  failures: number;
  /** `attempted`/`succeeded` count update targets (one per copy); these count distinct skills, which is what the toast names. */
  skillsAttempted: number;
  skillsSucceeded: number;
  /** The first failed target's message, so the toast can say why. */
  firstError: string | null;
}

const MAX_ERROR_LENGTH = 140;

/** "1 failed: <first error>" for the toast, or `undefined` when nothing failed. Counts skills, matching the toast title; a skill with any failed copy counts once. */
export function updateAllFailureMessage(tally: UpdateAllTally): string | undefined {
  if (tally.failures === 0) return undefined;
  const failedSkills = tally.skillsAttempted - tally.skillsSucceeded;
  if (!tally.firstError) return `${failedSkills} failed`;
  const reason =
    tally.firstError.length > MAX_ERROR_LENGTH
      ? `${tally.firstError.slice(0, MAX_ERROR_LENGTH - 1)}…`
      : tally.firstError;
  return `${failedSkills} failed: ${reason}`;
}

/**
 * Home's "Update all": a fork pulls upstream one at a time (no batched CLI
 * form for that path), while every other outdated owner flattens into one
 * `updateAllOwners` call - one IPC round trip and one rescan for the whole
 * batch, instead of one `updateSkill` round trip and rescan per skill.
 * `onProgress(done, total)` counts forks and owner targets in one sequence;
 * `updateAllOwners` reports how many of its own targets finished.
 */
export async function updateAllOutdatedSkills(
  skills: Pick<
    InstalledSkill,
    "name" | "deployments" | "source_kind" | "update_owner_ids" | "update_owners"
  >[],
  pullFork: (target: LifecycleTarget) => Promise<PullResult>,
  updateAllOwners: (
    targets: LifecycleTarget[],
    onOwnerDone: (done: number) => void,
  ) => Promise<UpdateAllOutcome>,
  onProgress?: (done: number, total: number) => void,
): Promise<UpdateAllTally> {
  const forks = skills.filter((skill) => skill.source_kind === "fork");
  const ownerTargets = skills.flatMap((skill) =>
    skill.source_kind === "fork" ? [] : skillUpdateOwnerTargets(skill),
  );
  const total = forks.length + ownerTargets.length;
  const ownerSkillNames = new Set(
    skills.flatMap((skill) =>
      skill.source_kind !== "fork" && skillUpdateOwnerTargets(skill).length > 0 ? [skill.name] : [],
    ),
  );
  const failedSkillNames = new Set<string>();
  const tally: UpdateAllTally = {
    attempted: total,
    succeeded: 0,
    failures: 0,
    skillsAttempted: forks.length + ownerSkillNames.size,
    skillsSucceeded: 0,
    firstError: null,
  };
  const fail = (count: number, message: string) => {
    tally.failures += count;
    tally.firstError ??= message;
  };
  onProgress?.(0, total);

  for (const [index, skill] of forks.entries()) {
    try {
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- update-all runs sequentially on purpose; concurrent `npx skills update` calls race on ~/.agents/.skill-lock.json
      await pullFork(lifecycleTargetForPark(skill));
      tally.succeeded += 1;
    } catch (error) {
      failedSkillNames.add(skill.name);
      fail(1, error instanceof Error ? error.message : String(error));
    }
    onProgress?.(index + 1, total);
  }

  if (ownerTargets.length > 0) {
    try {
      const outcome = await updateAllOwners(ownerTargets, (done) =>
        onProgress?.(forks.length + done, total),
      );
      // `errors` is keyed by skill name, so two failing owners of one
      // twice-installed skill collapse to one entry there; `items` carries
      // one entry per owner regardless, so count failures from `items`
      // instead (N1, review round 3).
      const failedItems = outcome.items.filter((item) => item.outcome === null);
      tally.succeeded += outcome.items.length - failedItems.length;
      for (const item of failedItems) failedSkillNames.add(item.skill);
      if (failedItems.length > 0) {
        const first = failedItems[0];
        fail(failedItems.length, outcome.errors[first.skill] ?? `${first.skill} failed`);
      }
    } catch (error) {
      for (const name of ownerSkillNames) failedSkillNames.add(name);
      fail(ownerTargets.length, error instanceof Error ? error.message : String(error));
    }
  }

  // Core can report one requested skill under two names, so the difference can go below zero.
  tally.skillsSucceeded = Math.max(0, tally.skillsAttempted - failedSkillNames.size);
  return tally;
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
