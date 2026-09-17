import type { TrialExpiryFailure } from "./skill-api";

export interface TrialExpiryFailureToast {
  id: string;
  title: string;
  description: string;
  duration: number;
}

export function trialExpiryFailureToast(failure: TrialExpiryFailure): TrialExpiryFailureToast {
  const scope =
    failure.scope === "project" && failure.project_path
      ? `project ${failure.project_path}`
      : failure.scope;
  const identity = JSON.stringify([failure.scope, failure.project_path, failure.name]);

  return {
    id: `trial-expiry-failed:${identity}`,
    title: failure.recovery_required
      ? `Trial expiry needs recovery: ${failure.name}`
      : `Trial expiry blocked: ${failure.name}`,
    description: failure.recovery_required
      ? `${scope}: ${failure.message} Skill Studio will attempt recovery before retrying.`
      : `${scope}: ${failure.message} The expiry will retry.`,
    duration: 15000,
  };
}
