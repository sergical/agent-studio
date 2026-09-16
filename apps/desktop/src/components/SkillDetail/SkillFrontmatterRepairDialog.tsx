// ============================================================================
// SkillFrontmatterRepairDialog - read-only YAML repair preview and authorized
// actions for one exact deployment.
// ============================================================================

import { useState } from "react";
import { useDocumentCancellation } from "../../hooks/useDocumentCancellation";
import { PatchDiff } from "@pierre/diffs/react";
import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@skill-studio/ui";
import { homeRelativePath, unifiedSkillMdDiff } from "@skill-studio/lib";
import type {
  FrontmatterRepairApplyMode,
  FrontmatterRepairPreview,
  LifecycleTarget,
} from "@skill-studio/lib";
import { applySkillFrontmatterRepair } from "../../lib/skill-api";
import { diffTheme } from "../../lib/theme";
import { useAppStore } from "../../store/appStore";

interface SkillFrontmatterRepairDialogProps {
  target: LifecycleTarget;
  preview: FrontmatterRepairPreview;
  onClose: () => void;
  onApplied: () => void;
  onEditManually: () => void;
}

export function SkillFrontmatterRepairDialog({
  target,
  preview,
  onClose,
  onApplied,
  onEditManually,
}: SkillFrontmatterRepairDialogProps) {
  const [applying, setApplying] = useState<FrontmatterRepairApplyMode | null>(null);
  const [applyError, setApplyError] = useState<string | null>(null);
  const cancellation = useDocumentCancellation();
  const addToast = useAppStore((state) => state.addToast);
  const theme = diffTheme(useAppStore((state) => state.resolvedTheme));
  const apply = (mode: FrontmatterRepairApplyMode) => {
    cancellation.reset();
    setApplyError(null);
    setApplying(mode);
    applySkillFrontmatterRepair(target, preview, mode, cancellation.onStarted)
      .then(() => {
        addToast({ type: "success", title: "YAML fixed" });
        onApplied();
        onClose();
      })
      .catch((error) => {
        const message = error instanceof Error ? error.message : "Unknown error";
        setApplyError(message);
        addToast({
          type: "error",
          title: "Couldn't fix YAML",
          message,
        });
      })
      .finally(() => {
        setApplying(null);
        cancellation.reset();
      });
  };

  return (
    <Dialog open onOpenChange={(open) => !open && applying === null && onClose()}>
      <DialogContent className="sm:max-w-3xl">
        <DialogHeader>
          <DialogTitle>Preview YAML fix</DialogTitle>
          <DialogDescription className="break-words">
            {preview.reason} This preview is for {homeRelativePath(preview.path)} ({preview.scope}).
            Nothing changes until you choose an action.
          </DialogDescription>
        </DialogHeader>
        <div className="min-w-0 max-h-[55vh] overflow-auto rounded-sm border border-border-subtle">
          <PatchDiff
            patch={unifiedSkillMdDiff(preview.original_content, preview.proposed_content)}
            options={{ theme, disableFileHeader: true }}
          />
        </div>
        {preview.allowed_apply_modes.includes("fix-installed-copy") && (
          <p className="m-0 text-small text-warning">
            Fix installed copy keeps managed ownership. A later update can overwrite this fix.
          </p>
        )}
        {applyError && (
          <p role="alert" className="m-0 break-words text-small text-destructive">
            Couldn't fix YAML: {applyError}
          </p>
        )}
        <DialogFooter className="sm:flex-wrap">
          <Button
            variant="outline"
            onClick={applying === null ? onClose : cancellation.cancel}
            disabled={applying !== null && (!cancellation.canCancel || cancellation.isCancelling)}
          >
            {cancellation.isCancelling ? "Stopping…" : applying === null ? "Cancel" : "Stop repair"}
          </Button>
          <Button
            variant="outline"
            onClick={() => {
              onClose();
              onEditManually();
            }}
            disabled={applying !== null}
          >
            Edit manually
          </Button>
          {preview.allowed_apply_modes.includes("fix-installed-copy") && (
            <Button
              variant="outline"
              onClick={() => apply("fix-installed-copy")}
              disabled={applying !== null}
            >
              Fix installed copy
            </Button>
          )}
          {preview.allowed_apply_modes.includes("apply-fix") && (
            <Button onClick={() => apply("apply-fix")} disabled={applying !== null}>
              Apply fix
            </Button>
          )}
          {preview.allowed_apply_modes.includes("fork-and-fix") && (
            <Button onClick={() => apply("fork-and-fix")} disabled={applying !== null}>
              Fork and fix (recommended)
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
