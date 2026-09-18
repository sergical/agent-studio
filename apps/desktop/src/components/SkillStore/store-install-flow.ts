// ============================================================================
// store-install-flow - the Skill Store install sequence, pulled out of
// SkillStoreInstallFlow.tsx so the trust-prompt branch (review round 2, B1)
// can be tested without a DOM. Each function takes the specific `skill-api.ts`
// calls it needs as arguments rather than importing `invoke` itself, matching
// the project rule that only `*-api.ts` names a Tauri command.
// ============================================================================

import type { AddSkillOperationEvent, AddSkillRequest } from "@skill-studio/lib";

interface StoreInstallStartApi {
  start: (operationId: string, request: AddSkillRequest) => Promise<AddSkillOperationEvent>;
  getOperation: (operationId: string) => Promise<AddSkillOperationEvent>;
}

interface StoreInstallTrustApi {
  confirmTrust: (
    operationId: string,
    retryOperationId: string,
    identity: string,
  ) => Promise<AddSkillOperationEvent>;
  getOperation: (operationId: string) => Promise<AddSkillOperationEvent>;
}

/** Starts a Store install and settles past `queued` to whatever phase `ops::install`
 * reaches next: `needs-trust` when the source is an unrecognized skills.sh repo, or a
 * later phase the operation already reached on its own. Callers render the trust
 * prompt off the returned phase instead of treating every non-success as a dead end. */
export async function startStoreInstall(
  operationId: string,
  request: AddSkillRequest,
  api: StoreInstallStartApi,
): Promise<AddSkillOperationEvent> {
  await api.start(operationId, request);
  return api.getOperation(operationId);
}

/** Confirms trust for the operation `startStoreInstall` paused on, then reads the
 * retry operation's settled state - the install only happens after this call, never
 * as a side effect of confirming. */
export async function confirmStoreInstallTrust(
  operationId: string,
  retryOperationId: string,
  identity: string,
  api: StoreInstallTrustApi,
): Promise<AddSkillOperationEvent> {
  const retry = await api.confirmTrust(operationId, retryOperationId, identity);
  return api.getOperation(retry.operation_id);
}

/** Declines the trust prompt by cancelling the paused operation. Callers must not call
 * `confirmStoreInstallTrust` afterward - this function alone is the whole decline path,
 * so nothing installs after it. */
export async function declineStoreInstallTrust(
  operationId: string,
  cancel: (operationId: string) => Promise<AddSkillOperationEvent>,
): Promise<AddSkillOperationEvent> {
  return cancel(operationId);
}
