// ============================================================================
// useStoreTrustStep - the Decline/Trust-and-retry handlers for a Store
// install's trust prompt, pulled out of `SkillStoreInstallFlow.tsx` so that
// component stays under react-doctor's giant-component threshold. Takes the
// specific state, refs, and callbacks it needs as arguments rather than
// importing them, so it stays a thin extraction and not a second owner of
// the component's state.
// ============================================================================

import type { Dispatch, RefObject, SetStateAction } from "react";
import {
  confirmStoreInstallTrust,
  declineStoreInstallTrust,
  parentProgressForPhase,
} from "./store-install-flow";
import type { SkillInstallCompletion } from "./InstallControls";
import type { AddSkillOperationEvent } from "@skill-studio/lib";
import type { Toast } from "@skill-studio/lib";

interface UseStoreTrustStepArgs {
  skillName: string;
  operation: AddSkillOperationEvent | undefined;
  trustBusy: boolean;
  operationIdRef: RefObject<string | undefined>;
  consumedIdRef: RefObject<string | undefined>;
  unlistenRef: RefObject<(() => void) | undefined>;
  setOperation: Dispatch<SetStateAction<AddSkillOperationEvent | undefined>>;
  setIsInstalling: (installing: boolean) => void;
  setTrustBusy: (busy: boolean) => void;
  onInstallStart: (skillName: string) => void;
  onInstallPaused: () => void;
  onInstallComplete: (result: SkillInstallCompletion) => void;
  addToast: (toast: Omit<Toast, "id">) => string;
  applyOperationEvent: (incoming: AddSkillOperationEvent) => void;
  // Passed in rather than imported from `skill-api.ts` directly, so this file
  // does not need the same layering exemption `SkillStoreInstallFlow.tsx` has
  // (plan.md section 7: components go through the store, not straight to IPC).
  cancelAddSkillOperation: (operationId: string) => Promise<AddSkillOperationEvent>;
  confirmAddSkillTrust: (
    operationId: string,
    retryOperationId: string,
    identity: string,
  ) => Promise<AddSkillOperationEvent>;
  getAddSkillOperation: (operationId: string) => Promise<AddSkillOperationEvent>;
}

/** Returns the trust-prompt handlers `StoreInstallFooter` calls: decline (cancels the
 * paused operation) and trust-and-retry (confirms trust, then installs under a
 * client-generated retry id). Behavior is unchanged from before the extraction. */
export function useStoreTrustStep({
  skillName,
  operation,
  trustBusy,
  operationIdRef,
  consumedIdRef,
  unlistenRef,
  setOperation,
  setIsInstalling,
  setTrustBusy,
  onInstallStart,
  onInstallPaused,
  onInstallComplete,
  addToast,
  applyOperationEvent,
  cancelAddSkillOperation,
  confirmAddSkillTrust,
  getAddSkillOperation,
}: UseStoreTrustStepArgs) {
  const handleDeclineTrust = async () => {
    const operationId = operationIdRef.current;
    unlistenRef.current?.();
    unlistenRef.current = undefined;
    operationIdRef.current = undefined;
    setOperation(undefined);
    setIsInstalling(false);
    if (parentProgressForPhase("cancelled") === "clear") onInstallPaused();
    if (!operationId) return;
    try {
      await declineStoreInstallTrust(operationId, cancelAddSkillOperation);
    } catch (error) {
      addToast({
        type: "error",
        title: "Could not decline repository trust",
        message: error instanceof Error ? error.message : "Unknown error",
      });
    }
  };

  const handleTrustAndRetry = async () => {
    const operationId = operationIdRef.current;
    const identity = operation?.untrusted_source?.identity;
    if (!operationId || !identity || trustBusy) return;
    setTrustBusy(true);
    // The parent's `InstallProgressModal` was cleared for `needs-trust`
    // (`applyOperationEvent`); show it again so the retry has the same
    // "Installing…" feedback the initial attempt had.
    onInstallStart(skillName);
    try {
      const retryOperationId = crypto.randomUUID();
      // The retry id is client-generated, so it is tracked before the await
      // below, not after: a terminal event for the retry op delivered while
      // this call is in flight would otherwise be dropped by the id filter
      // in `applyOperationEvent` (review round 3, N1).
      operationIdRef.current = retryOperationId;
      consumedIdRef.current = undefined;
      const settled = await confirmStoreInstallTrust(operationId, retryOperationId, identity, {
        confirmTrust: confirmAddSkillTrust,
        getOperation: getAddSkillOperation,
      });
      applyOperationEvent(settled);
    } catch (error) {
      operationIdRef.current = operationId;
      setIsInstalling(false);
      onInstallPaused();
      onInstallComplete({
        success: false,
        error: error instanceof Error ? error.message : "Trust confirmation failed",
        skillName,
      });
    }
    setTrustBusy(false);
  };

  return { handleDeclineTrust, handleTrustAndRetry };
}
