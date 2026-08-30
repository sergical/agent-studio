// ============================================================================
// InstallProgressModal - Shows installation progress
// ============================================================================

import { Dialog, DialogContent, DialogTitle } from "@skill-studio/ui";
import type { InstallProgressState } from "@skill-studio/lib";

interface InstallProgressModalProps {
  progress: InstallProgressState;
  onClose: () => void;
}

export function InstallProgressModal({ progress, onClose }: InstallProgressModalProps) {
  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent
        className="w-[400px] max-w-[calc(100%-2rem)] gap-0 rounded-lg border border-border bg-bg-elevated p-0 shadow-lg"
        aria-label={`Installing ${progress.skillName}`}
      >
        <div className="flex items-center justify-between border-b border-border px-5 py-4">
          <DialogTitle className="m-0 text-pretty text-balance text-emphasis font-semibold text-text-primary">
            Installing {progress.skillName}
          </DialogTitle>
        </div>

        <div className="flex flex-col items-center p-8 py-8">
          <span className="mb-4 size-8 animate-spin rounded-full border-[3px] border-border border-t-accent" />
          <p className="m-0 mb-1 text-body font-medium text-text-primary">{progress.stage}</p>
          <p className="m-0 text-small text-text-tertiary">{progress.message}</p>

          {progress.percent !== undefined && (
            <div className="mt-5 h-1 w-full overflow-hidden rounded-full bg-bg-tertiary">
              <div
                className="h-full rounded-full bg-accent transition-[width] duration-300 ease-out"
                style={{ width: `${progress.percent}%` }}
              />
            </div>
          )}

          {progress.error && (
            <div className="mt-4 w-full rounded-sm bg-error-soft p-3">
              <p className="m-0 text-small text-error">{progress.error}</p>
            </div>
          )}
        </div>
      </DialogContent>
    </Dialog>
  );
}
