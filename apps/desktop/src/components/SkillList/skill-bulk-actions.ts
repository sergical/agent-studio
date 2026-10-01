// ============================================================================
// Skill Studio - Bulk actions for the skill list's selection bar
// Which selected skills each bulk action can run on (and why the rest are
// skipped), a sequential runner, and the one toast a finished action reports.
// Each action reuses the single-skill availability rules, so the bar never
// disagrees with the skill page.
// ============================================================================

import type {
  ForkRecord,
  InstalledSkill,
  InvocationPolicy,
  LifecycleTarget,
  PullResult,
  Toast,
  UpdateAllOutcome,
} from "@skill-studio/lib";
import {
  conflictedSkillsNote,
  forkThenPull,
  excludeForkedOwner,
  forkTargetForSkill,
  skillMutableLifecycleScopes,
  skillParkVerb,
  skillRemovalAvailability,
  skillUpdateAvailability,
  skillUpdateOwnerTargets,
} from "../../lib/skill-lifecycle-target";
import {
  INVOCATION_POLICY_OPTIONS,
  invocationFilesForSkill,
} from "../SkillDetail/skill-location-status";

export type BulkAction =
  | { kind: "park" }
  | { kind: "unpark" }
  | { kind: "invocation"; policy: InvocationPolicy }
  | { kind: "update" }
  | { kind: "remove" };

interface BulkSkipped {
  skill: InstalledSkill;
  /** A phrase that reads after a count: "2 already parked". */
  reason: string;
}

export interface BulkPlan {
  applicable: InstalledSkill[];
  skipped: BulkSkipped[];
}

export interface BulkFailure {
  skill: InstalledSkill;
  error: string;
}

export interface BulkRunResult {
  succeeded: InstalledSkill[];
  failed: BulkFailure[];
  /** Names of forked skills whose pull left conflict markers; set only for an update. */
  conflicted?: string[];
}

/** Why `skill` cannot take `action`, or `null` when it can. */
function skipReason(skill: InstalledSkill, action: BulkAction): string | null {
  switch (action.kind) {
    case "park":
    case "unpark": {
      const verb = skillParkVerb(skill);
      if (verb === null) return "no Global Universal folder";
      if (action.kind === "park") return verb === "Park" ? null : "already parked";
      return verb === "Unpark" ? null : "not parked";
    }
    case "invocation": {
      const files = invocationFilesForSkill(skill);
      if (files.some((file) => file.editable)) return null;
      return files.length === 0 ? "no SKILL.md to edit" : "no editable file";
    }
    case "update": {
      if (skillMutableLifecycleScopes(skill).length === 0) return "no managed copy";
      if (bulkUpdateTargets(skill).length > 0) return null;
      return skillUpdateOwnerTargets(skill).length === 0
        ? "no update available"
        : "needs a specific location";
    }
    case "remove": {
      const scopes = skillMutableLifecycleScopes(skill);
      if (scopes.length === 0) return "no removable copy";
      // Every location must be removable, or "removed" would leave a copy behind.
      return bulkRemovalTargets(skill).length === scopes.length ? null : "needs a specific copy";
    }
  }
}

/** Splits `skills` into the ones `action` can run on and the ones it skips, each with a reason. */
export function planBulkAction(skills: InstalledSkill[], action: BulkAction): BulkPlan {
  const plan: BulkPlan = { applicable: [], skipped: [] };
  for (const skill of skills) {
    const reason = skipReason(skill, action);
    if (reason === null) plan.applicable.push(skill);
    else plan.skipped.push({ skill, reason });
  }
  return plan;
}

/** The update targets of a skill `planBulkAction` accepted for "update": one per location with an update. */
export function bulkUpdateTargets(skill: InstalledSkill): LifecycleTarget[] {
  return skillMutableLifecycleScopes(skill).flatMap((selection) => {
    const availability = skillUpdateAvailability(skill, selection);
    return availability.available ? [availability.target] : [];
  });
}

/** The removal targets of a skill `planBulkAction` accepted for "remove": one per location. */
export function bulkRemovalTargets(skill: InstalledSkill): LifecycleTarget[] {
  return skillMutableLifecycleScopes(skill).flatMap((selection) => {
    const availability = skillRemovalAvailability(skill, selection);
    return availability.available ? [availability.preview.target] : [];
  });
}

/**
 * Runs `run` for each skill one after another - every core op takes an
 * exclusive lease, so parallel calls would only queue or collide. A rejected
 * call is recorded and the rest still run.
 */
export async function runBulkSequentially(
  skills: InstalledSkill[],
  run: (skill: InstalledSkill) => Promise<void>,
  onProgress?: (current: number, total: number) => void,
): Promise<BulkRunResult> {
  const result: BulkRunResult = { succeeded: [], failed: [] };
  for (const [index, skill] of skills.entries()) {
    onProgress?.(index + 1, skills.length);
    try {
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- each core op takes an exclusive lease, so the calls must not overlap
      await run(skill);
      result.succeeded.push(skill);
    } catch (error) {
      result.failed.push({
        skill,
        error: error instanceof Error ? error.message : "Unknown error",
      });
    }
  }
  return result;
}

/**
 * Turns an batched update outcome into a run result: an item without an
 * outcome failed, and a skill with several location items fails if any one did.
 */
export function bulkUpdateResult(
  skills: InstalledSkill[],
  outcome: { items: { skill: string; outcome: unknown }[]; errors: Record<string, string> },
): BulkRunResult {
  const result: BulkRunResult = { succeeded: [], failed: [] };
  for (const skill of skills) {
    const items = outcome.items.filter((item) => item.skill === skill.name);
    const error = outcome.errors[skill.name];
    if (error !== undefined || items.some((item) => item.outcome === null)) {
      result.failed.push({ skill, error: error ?? "Update failed without an error message." });
    } else if (items.length > 0) {
      result.succeeded.push(skill);
    } else {
      result.failed.push({ skill, error: "The update returned no result for this skill." });
    }
  }
  return result;
}

/**
 * The list's bulk Update: each skill in `forkNames` is forked and pulled
 * (keeping the user's edits), the rest go through one batched update call.
 * One result covers both, with the skills whose pull left conflict markers.
 */
export async function runBulkUpdate(
  skills: InstalledSkill[],
  forkNames: ReadonlySet<string>,
  deps: {
    fork: (target: LifecycleTarget) => Promise<ForkRecord>;
    pullFork: (target: LifecycleTarget) => Promise<PullResult>;
    updateAll: (
      targets: LifecycleTarget[],
      onProgress: (done: number, total: number) => void,
    ) => Promise<UpdateAllOutcome>;
  },
  onProgress: (done: number, total: number) => void,
): Promise<BulkRunResult> {
  const forked = skills.filter((skill) => forkNames.has(skill.name));
  const rest = skills.filter((skill) => !forkNames.has(skill.name));
  const result: BulkRunResult = { succeeded: [], failed: [] };
  const conflicted: string[] = [];
  // A forked skill's other owners still get the normal update.
  const forkedOthers = new Map(
    forked.map((skill) => [skill, excludeForkedOwner(skill, bulkUpdateTargets(skill))]),
  );
  let total =
    forked.length +
    rest.flatMap(bulkUpdateTargets).length +
    [...forkedOthers.values()].reduce((sum, targets) => sum + targets.length, 0);
  const forkFailed = new Set<InstalledSkill>();
  for (const [index, skill] of forked.entries()) {
    try {
      // react-doctor-disable-next-line react-doctor/async-await-in-loop -- each fork and pull takes an exclusive lease, so the calls must not overlap
      const pull = await forkThenPull(forkTargetForSkill(skill), deps.fork, deps.pullFork);
      if (pull.conflicts.length > 0) conflicted.push(skill.name);
      result.succeeded.push(skill);
    } catch (error) {
      forkFailed.add(skill);
      result.failed.push({
        skill,
        error: error instanceof Error ? error.message : "Unknown error",
      });
    }
    onProgress(index + 1, total);
  }
  // A skill whose fork failed keeps all its owners untouched, so its other copies are not updated either.
  const forkedToBatch = forked.filter(
    (skill) => !forkFailed.has(skill) && (forkedOthers.get(skill) ?? []).length > 0,
  );
  const batched = [...rest, ...forkedToBatch];
  const batchTargets = [
    ...rest.flatMap(bulkUpdateTargets),
    ...forkedToBatch.flatMap((skill) => forkedOthers.get(skill) ?? []),
  ];
  // A failed fork's other copies leave the total, so progress still reaches it.
  const plannedTotal = total;
  total = forked.length + batchTargets.length;
  if (total !== plannedTotal) onProgress(forked.length, total);
  if (batched.length > 0) {
    const outcome = await deps.updateAll(batchTargets, (done) =>
      onProgress(forked.length + done, total),
    );
    const batchResult = bulkUpdateResult(batched, outcome);
    for (const skill of batchResult.succeeded) {
      if (!forkedOthers.has(skill)) result.succeeded.push(skill);
    }
    for (const failure of batchResult.failed) {
      if (!forkedOthers.has(failure.skill)) {
        result.failed.push(failure);
      } else {
        // The fork and pull went through; only another copy failed. Count it as failed, once, with that reason.
        result.succeeded = result.succeeded.filter((skill) => skill !== failure.skill);
        result.failed.push({
          skill: failure.skill,
          error: `Forked and updated, but another copy failed: ${failure.error}`,
        });
      }
    }
  }
  if (conflicted.length > 0) result.conflicted = conflicted;
  return result;
}

function policyLabel(policy: InvocationPolicy): string {
  return (
    INVOCATION_POLICY_OPTIONS.find((option) => option.value === policy)?.label ?? policy
  ).toLowerCase();
}

function skillCount(count: number): string {
  return `${count} skill${count === 1 ? "" : "s"}`;
}

/** "Parking 5 skills…" for a batched action; "Removing 2 of 5…" while removal runs one by one. */
export function bulkProgressLabel(action: BulkAction, current: number, total: number): string {
  const verb = {
    park: "Parking",
    unpark: "Unparking",
    invocation: "Setting invocation on",
    update: "Updating",
    remove: "Removing",
  }[action.kind];
  return action.kind === "remove"
    ? `${verb} ${current} of ${total}…`
    : `${verb} ${skillCount(total)}…`;
}

/** "Updating 12 of 80…" as `update_all_skills` reports each finished location. */
export function bulkUpdateProgressLabel(done: number, total: number): string {
  return `Updating ${done} of ${total}…`;
}

/** "2 already parked, 1 no editable file" - the skipped skills grouped by reason. */
export function describeSkipped(skipped: BulkSkipped[]): string {
  const counts = new Map<string, number>();
  for (const { reason } of skipped) counts.set(reason, (counts.get(reason) ?? 0) + 1);
  return [...counts].map(([reason, count]) => `${count} ${reason}`).join(", ");
}

/** The tooltip for a bar button whose action applies to none of the selection, or `null` when it can run. */
export function bulkDisabledReason(action: BulkAction, plan: BulkPlan): string | null {
  if (plan.applicable.length > 0) return null;
  const verb = {
    park: "park",
    unpark: "unpark",
    invocation: "change invocation on",
    update: "update",
    remove: "remove",
  }[action.kind];
  return plan.skipped.length === 0
    ? `Select skills to ${verb}`
    : `Nothing to ${verb}: ${describeSkipped(plan.skipped)}`;
}

function pastTitle(action: BulkAction): string {
  switch (action.kind) {
    case "park":
      return "Parked";
    case "unpark":
      return "Unparked";
    case "invocation":
      return `Set ${policyLabel(action.policy)} on`;
    case "update":
      return "Updated";
    case "remove":
      return "Removed";
  }
}

/**
 * The one toast for a finished bulk action: how many skills changed, how many
 * were skipped and why, and which ones failed with their errors.
 */
export function bulkActionToast(
  action: BulkAction,
  plan: BulkPlan,
  result: BulkRunResult,
): Omit<Toast, "id"> {
  const total = plan.applicable.length + plan.skipped.length;
  const changed = result.succeeded.length;
  const parts: string[] = [];
  if (plan.skipped.length > 0) parts.push(describeSkipped(plan.skipped));
  if (result.failed.length > 0) parts.push(`${result.failed.length} failed`);
  const subject = changed === total ? skillCount(total) : `${changed} of ${skillCount(total)}`;
  const title = [`${pastTitle(action)} ${subject}`, ...parts].join(" · ");
  const conflictNote = conflictedSkillsNote(result.conflicted ?? []);
  if (result.failed.length === 0) {
    return conflictNote
      ? { type: "warning", title, message: conflictNote }
      : { type: "success", title };
  }
  const failureMessage = result.failed
    .map(({ skill, error }) => `${skill.name}: ${error}`)
    .join("; ");
  return {
    type: changed === 0 ? "error" : "warning",
    title,
    message: conflictNote ? `${failureMessage}. ${conflictNote}` : failureMessage,
  };
}
