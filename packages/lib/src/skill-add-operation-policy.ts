// ============================================================================
// Skill Studio - skill-add-operation-policy
// Which Add Skill operation events to keep, and which UI controls they allow
// ============================================================================

import type { AddSkillOperationEvent, AddSkillOperationPhase } from "./skill-add-operation-types";

const TERMINAL_PHASES = new Set<AddSkillOperationPhase>([
  "completed",
  "failed",
  "cancelled",
  "timed-out",
]);

const CANCELLABLE_PHASES = new Set<AddSkillOperationPhase>([
  "queued",
  "validating",
  "fetching",
  "installing",
  "finalizing",
]);

/** True when the operation will not emit another phase. */
export function isAddSkillOperationTerminal(phase: AddSkillOperationPhase): boolean {
  return TERMINAL_PHASES.has(phase);
}

/** True while Cancel should stay enabled. Needs-trust uses Close, not Cancel. */
export function isAddSkillOperationCancellable(phase: AddSkillOperationPhase): boolean {
  return CANCELLABLE_PHASES.has(phase);
}

/**
 * Keep `incoming` only when it belongs to `operationId` and its sequence is
 * strictly greater than the current event for that same id. A leftover event
 * from a parent trust operation is replaced, not mixed.
 */
export function selectNewerAddSkillOperationEvent(
  current: AddSkillOperationEvent | undefined,
  incoming: AddSkillOperationEvent,
  operationId: string,
): AddSkillOperationEvent | undefined {
  if (incoming.operation_id !== operationId) return current;
  if (!current || current.operation_id !== operationId) return incoming;
  if (incoming.sequence > current.sequence) return incoming;
  return current;
}

/** True once for each terminal operation id. Needs-trust is never consumed. */
export function shouldConsumeAddSkillOperation(
  status: AddSkillOperationEvent,
  consumedOperationId: string | undefined,
): boolean {
  if (consumedOperationId === status.operation_id) return false;
  if (status.phase === "needs-trust") return false;
  return isAddSkillOperationTerminal(status.phase);
}

/** Progress line: phase title plus batch "2 of 5" when present. */
export function addSkillOperationProgressCopy(status: AddSkillOperationEvent): string {
  if (status.item) {
    return `${addSkillOperationStatusTitle(status.phase)} · ${status.item.current} of ${status.item.total}`;
  }
  return status.message || addSkillOperationStatusTitle(status.phase);
}

/** Distinct failed / cancelled / timed-out copy. */
export function addSkillOperationTerminalCopy(status: AddSkillOperationEvent): string {
  switch (status.phase) {
    case "cancelled":
      return "Cancelled";
    case "timed-out":
      return "Timed out";
    default:
      return status.error ?? status.message;
  }
}

export type AddSkillFinishAction =
  | { kind: "error"; error: string }
  | {
      kind: "success";
      title: string;
      message?: string;
      openName?: string;
      failedTitle?: string;
      failedMessage?: string;
    };

/**
 * Preserve the pre-operation Add Skill result toasts: open the one installed
 * skill, warn on partial batch failure, and surface a lone failure as error.
 */
export function addSkillFinishAction(status: AddSkillOperationEvent): AddSkillFinishAction {
  if (status.phase === "cancelled" || status.phase === "timed-out" || status.phase === "failed") {
    return { kind: "error", error: addSkillOperationTerminalCopy(status) };
  }
  if (status.outcomes) {
    const installed = status.outcomes.filter((outcome) => outcome.result);
    const failed = status.outcomes.filter((outcome) => outcome.error);
    if (installed.length === 0) {
      return {
        kind: "error",
        error:
          failed.map((item) => `${item.name}: ${item.error}`).join("; ") || "Nothing was installed",
      };
    }
    return {
      kind: "success",
      title: `Added ${installed.length} skill${installed.length !== 1 ? "s" : ""}`,
      message:
        installed.flatMap((outcome) => outcome.result?.warning ?? []).join("; ") || undefined,
      openName: installed.length === 1 ? installed[0].name : undefined,
      failedTitle:
        failed.length > 0
          ? `${failed.length} skill${failed.length !== 1 ? "s" : ""} failed`
          : undefined,
      failedMessage:
        failed.length > 0
          ? failed.map((item) => `${item.name}: ${item.error}`).join("; ")
          : undefined,
    };
  }
  if (status.result) {
    return {
      kind: "success",
      title: `Added ${status.result.name}`,
      message: status.result.warning ?? undefined,
      openName: status.result.name,
    };
  }
  return { kind: "error", error: status.error ?? status.message };
}

/** User-facing title for a terminal or needs-trust phase. */
export function addSkillOperationStatusTitle(phase: AddSkillOperationPhase): string {
  switch (phase) {
    case "queued":
      return "Queued";
    case "validating":
      return "Checking source";
    case "fetching":
      return "Fetching";
    case "installing":
      return "Installing";
    case "finalizing":
      return "Finishing";
    case "reconciling":
      return "Updating list";
    case "needs-trust":
      return "Trust this repository?";
    case "completed":
      return "Added";
    case "failed":
      return "Add failed";
    case "cancelled":
      return "Cancelled";
    case "timed-out":
      return "Timed out";
  }
}
