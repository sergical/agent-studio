// ============================================================================
// Skill Studio - Bulk actions for the skill list's selection bar
// Which selected skills each bulk action can run on (and why the rest are
// skipped), a sequential runner, and the one toast a finished action reports.
// Each action reuses the single-skill availability rules, so the bar never
// disagrees with the skill page.
// ============================================================================

import type { InstalledSkill, InvocationPolicy, LifecycleTarget, Toast } from "@skill-studio/lib";
import {
  skillLifecycleScopeSelection,
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

interface BulkPlan {
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
      const selection = skillLifecycleScopeSelection(skill);
      if (!selection) return "no managed copy";
      if (skillUpdateAvailability(skill, selection).available) return null;
      return skillUpdateOwnerTargets(skill).length === 0
        ? "no update available"
        : "needs a specific location";
    }
    case "remove": {
      const selection = skillLifecycleScopeSelection(skill);
      if (!selection) return "no removable copy";
      return skillRemovalAvailability(skill, selection).available ? null : "needs a specific copy";
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

/** The update target of a skill `planBulkAction` accepted for "update". */
export function bulkUpdateTarget(skill: InstalledSkill): LifecycleTarget | null {
  const selection = skillLifecycleScopeSelection(skill);
  const availability = selection && skillUpdateAvailability(skill, selection);
  return availability?.available ? availability.target : null;
}

/** The removal target of a skill `planBulkAction` accepted for "remove". */
export function bulkRemovalTarget(skill: InstalledSkill): LifecycleTarget | null {
  const selection = skillLifecycleScopeSelection(skill);
  const availability = selection && skillRemovalAvailability(skill, selection);
  return availability?.available ? availability.preview.target : null;
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

/** Turns an `updateAllSkills` outcome into a run result: an item without an outcome failed. */
export function bulkUpdateResult(
  skills: InstalledSkill[],
  outcome: { items: { skill: string; outcome: unknown }[]; errors: Record<string, string> },
): BulkRunResult {
  const result: BulkRunResult = { succeeded: [], failed: [] };
  const itemsBySkill = new Map(outcome.items.map((item) => [item.skill, item]));
  for (const skill of skills) {
    const item = itemsBySkill.get(skill.name);
    const error = outcome.errors[skill.name];
    if (error !== undefined || (item && item.outcome === null)) {
      result.failed.push({ skill, error: error ?? "Update failed without an error message." });
    } else if (item) {
      result.succeeded.push(skill);
    } else {
      result.failed.push({ skill, error: "The update returned no result for this skill." });
    }
  }
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
  if (result.failed.length === 0) return { type: "success", title };
  return {
    type: changed === 0 ? "error" : "warning",
    title,
    message: result.failed.map(({ skill, error }) => `${skill.name}: ${error}`).join("; "),
  };
}
