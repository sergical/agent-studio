// ============================================================================
// Skill Studio - useAddSkillOperation
// Subscribe before start, then catch up with get-status after remount
// ============================================================================

import { getAddSkillOperation, onAddSkillOperation } from "../lib/skill-api";
import type { AddSkillOperationEvent } from "@skill-studio/lib";
import { selectNewerAddSkillOperationEvent } from "@skill-studio/lib";

interface AddSkillOperationListen {
  isCancelled: () => boolean;
  listen: typeof onAddSkillOperation;
  onEvent: (event: AddSkillOperationEvent) => void;
}

interface AddSkillOperationSubscription extends AddSkillOperationListen {
  read: typeof getAddSkillOperation;
  operationId: string;
  onError: (message: string) => void;
}

/**
 * Register the Add Skill listener and return before any start or status
 * read, so the sheet can subscribe with a known id first.
 */
export async function listenForAddSkillOperation({
  isCancelled,
  listen,
  onEvent,
}: AddSkillOperationListen): Promise<(() => void) | undefined> {
  const unlisten = await listen((candidate) => {
    if (!isCancelled()) onEvent(candidate);
  });
  if (isCancelled()) {
    unlisten();
    return undefined;
  }
  return unlisten;
}

/**
 * Register the Add Skill listener before reading status, and drop a late
 * registration after unmount so a remount cannot apply a stale snapshot.
 */
export async function startAddSkillOperationSubscription({
  isCancelled,
  listen,
  read,
  operationId,
  onEvent,
  onError,
}: AddSkillOperationSubscription): Promise<(() => void) | undefined> {
  let unlisten: (() => void) | undefined;
  try {
    unlisten = await listenForAddSkillOperation({ isCancelled, listen, onEvent });
    if (!unlisten || isCancelled()) return unlisten;
    const initial = await read(operationId);
    if (isCancelled()) {
      unlisten();
      return undefined;
    }
    onEvent(initial);
    return unlisten;
  } catch (error) {
    unlisten?.();
    if (!isCancelled()) {
      onError(error instanceof Error ? error.message : "Failed to read add skill status");
    }
    return undefined;
  }
}

/** Fold an incoming event into local state, dropping stale sequences. */
export function applyAddSkillOperationEvent(
  current: AddSkillOperationEvent | undefined,
  incoming: AddSkillOperationEvent,
  operationId: string | undefined,
): AddSkillOperationEvent | undefined {
  if (!operationId) return current;
  return selectNewerAddSkillOperationEvent(current, incoming, operationId);
}
