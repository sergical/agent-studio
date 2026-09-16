// ============================================================================
// InstallProgressModal - Shows installation progress
// ============================================================================

import { Button, Dialog, DialogContent, DialogTitle, Progress } from "@skill-studio/ui";
import {
  addSkillOperationProgressCopy,
  addSkillOperationStatusTitle,
  isAddSkillOperationCancellable,
  isAddSkillOperationTerminal,
} from "@skill-studio/lib";
import type { AddSkillOperationEvent } from "@skill-studio/lib";

interface InstallProgressModalProps {
  skillName: string;
  operation: AddSkillOperationEvent;
  statusError?: string;
  isCancelling: boolean;
  onClose: () => void;
  onCancel: () => void;
}

export function InstallProgressModal({
  skillName,
  operation,
  statusError,
  isCancelling,
  onClose,
  onCancel,
}: InstallProgressModalProps) {
  const terminal = isAddSkillOperationTerminal(operation.phase);
  const cancellable = isAddSkillOperationCancellable(operation.phase);
  const error =
    terminal && operation.phase !== "completed"
      ? (operation.error ?? operation.message)
      : undefined;

  return (
    <Dialog open onOpenChange={(open) => !open && (terminal ? onClose() : onCancel())}>
      <DialogContent
        className="w-[400px] max-w-[calc(100%-2rem)] gap-0 rounded-lg border border-border bg-bg-elevated p-0 shadow-lg"
        aria-label={`Installing ${skillName}`}
      >
        <div className="flex items-center justify-between border-b border-border px-5 py-4">
          <DialogTitle className="m-0 text-pretty text-balance text-emphasis font-semibold text-text-primary">
            Installing {skillName}
          </DialogTitle>
        </div>

        <div className="flex flex-col items-center p-8 py-8">
          {!terminal && (
            <span className="mb-4 size-8 animate-spin rounded-full border-[3px] border-border border-t-accent" />
          )}
          <p className="m-0 mb-1 text-body font-medium text-text-primary">
            {isCancelling ? "Cancelling…" : addSkillOperationStatusTitle(operation.phase)}
          </p>
          <p className="m-0 text-small text-text-tertiary">
            {isCancelling
              ? "Waiting for installation to finish."
              : addSkillOperationProgressCopy(operation)}
          </p>

          {operation.item && (
            <Progress
              value={(operation.item.current / operation.item.total) * 100}
              className="mt-5"
            />
          )}

          {error && (
            <div className="mt-4 w-full rounded-sm bg-error-soft p-3">
              <p className="m-0 text-small text-error">{error}</p>
            </div>
          )}
          {statusError && (
            <div className="mt-4 w-full rounded-sm bg-warning-soft p-3">
              <p className="m-0 text-small text-warning">{statusError}</p>
            </div>
          )}

          <div className="mt-6 flex w-full justify-end gap-2">
            {terminal ? (
              <Button onClick={onClose}>Close</Button>
            ) : (
              <Button
                variant="outline"
                onClick={onCancel}
                disabled={(!cancellable && operation.phase !== "needs-trust") || isCancelling}
              >
                {isCancelling
                  ? "Cancelling…"
                  : operation.phase === "needs-trust"
                    ? "Close"
                    : "Cancel"}
              </Button>
            )}
          </div>
        </div>
      </DialogContent>
    </Dialog>
  );
}
