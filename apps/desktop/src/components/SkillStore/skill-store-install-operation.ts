// ============================================================================
// Skill Studio - store install operation
// Owns one Browse install operation while its stable SkillStore parent lives
// ============================================================================

import {
  applyAddSkillOperationEvent,
  listenForAddSkillOperation,
} from "../../hooks/useAddSkillOperation";
import type { AddSkillOperationEvent, AddSkillRequest } from "@skill-studio/lib";

interface StoreInstallOperationApi {
  listen: (callback: (event: AddSkillOperationEvent) => void) => Promise<() => void>;
  start: (operationId: string, request: AddSkillRequest) => Promise<AddSkillOperationEvent>;
  read: (operationId: string) => Promise<AddSkillOperationEvent>;
  cancel: (operationId: string) => Promise<AddSkillOperationEvent>;
}

interface StoreInstallOperationCallbacks {
  onEvent: (event: AddSkillOperationEvent) => void;
  onStatusError: (message: string) => void;
}

interface StoreInstallOperationOptions extends StoreInstallOperationCallbacks {
  api: StoreInstallOperationApi;
  createOperationId: () => string;
}

/** Coordinates a single operation without letting late IPC callbacks own a newer install. */
export function createStoreInstallOperationController({
  api,
  createOperationId,
  onEvent,
  onStatusError,
}: StoreInstallOperationOptions) {
  let operationId: string | undefined;
  let unlisten: (() => void) | undefined;
  let latest: AddSkillOperationEvent | undefined;

  const isCurrent = (candidate: string) => operationId === candidate;
  const clear = (candidate?: string) => {
    if (candidate && !isCurrent(candidate)) return;
    operationId = undefined;
    latest = undefined;
    unlisten?.();
    unlisten = undefined;
  };

  const apply = (incoming: AddSkillOperationEvent, id: string) => {
    if (!latest) return;
    const next = applyAddSkillOperationEvent(latest, incoming, id);
    if (next && next !== latest && isCurrent(id)) {
      latest = next;
      onEvent(next);
    }
  };

  return {
    activeOperationId: () => operationId,

    async start(request: AddSkillRequest) {
      if (operationId) return;
      const id = createOperationId();
      operationId = id;
      const queued: AddSkillOperationEvent = {
        operation_id: id,
        sequence: 0,
        phase: "queued",
        message: "Starting installation…",
      };
      latest = queued;
      onEvent(queued);

      let registered: (() => void) | undefined;
      try {
        registered = await listenForAddSkillOperation({
          isCancelled: () => !isCurrent(id),
          listen: api.listen,
          onEvent: (incoming) => apply(incoming, id),
        });
      } catch (error) {
        if (!isCurrent(id)) return;
        onEvent({
          operation_id: id,
          sequence: Number.MAX_SAFE_INTEGER,
          phase: "failed",
          message: "Installation could not start",
          error:
            error instanceof Error ? error.message : "Failed to subscribe to installation status",
        });
        return;
      }
      if (!isCurrent(id)) return;
      unlisten = registered;

      let started: AddSkillOperationEvent;
      try {
        started = await api.start(id, request);
      } catch (error) {
        if (!isCurrent(id)) return;
        onEvent({
          operation_id: id,
          sequence: Number.MAX_SAFE_INTEGER,
          phase: "failed",
          message: "Installation could not start",
          error:
            error instanceof Error ? error.message : "Install failed without an error message.",
        });
        return;
      }
      if (!isCurrent(id)) {
        void api.cancel(id).catch(() => undefined);
        return;
      }
      apply(started, id);

      try {
        const snapshot = await api.read(id);
        apply(snapshot, id);
      } catch (error) {
        if (!isCurrent(id)) return;
        onStatusError(
          error instanceof Error ? error.message : "Failed to read installation status",
        );
      }
    },

    async cancel() {
      const id = operationId;
      if (!id) return;
      const acknowledgement = await api.cancel(id);
      apply(acknowledgement, id);
    },

    release(id: string) {
      clear(id);
    },

    dispose() {
      const id = operationId;
      clear();
      if (id) void api.cancel(id).catch(() => undefined);
    },
  };
}
