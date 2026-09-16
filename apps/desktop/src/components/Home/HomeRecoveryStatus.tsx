import type { InterruptedSkillEventsStatus } from "../../hooks/useInterruptedSkillEvents";

interface HomeRecoveryStatusProps {
  onViewActivity: () => void;
  onRetry: () => void;
  status: InterruptedSkillEventsStatus;
}

export function canShowAllClear(
  hasInventoryIssues: boolean,
  status: InterruptedSkillEventsStatus,
): boolean {
  return !hasInventoryIssues && status.kind === "ready" && !status.hasInterrupted;
}

export function HomeRecoveryStatus({ onViewActivity, onRetry, status }: HomeRecoveryStatusProps) {
  if (status.kind === "ready" && status.hasInterrupted) {
    return (
      <div
        role="alert"
        className="rounded-md border border-warning bg-warning-soft px-3 py-2 text-small text-text-primary"
      >
        <span className="font-medium">Some skill changes need review.</span>{" "}
        <button className="text-accent hover:underline" onClick={onViewActivity}>
          View Activity
        </button>
      </div>
    );
  }

  if (status.kind === "unavailable") {
    return (
      <div
        role="status"
        className="rounded-md border border-warning bg-warning-soft px-3 py-2 text-small text-text-primary"
      >
        Could not check interrupted skill changes.{" "}
        <button className="text-accent hover:underline" onClick={onRetry}>
          Retry
        </button>
      </div>
    );
  }

  if (status.kind === "loading") {
    return (
      <p role="status" className="text-small text-text-tertiary">
        Checking interrupted skill changes…
      </p>
    );
  }

  return null;
}
