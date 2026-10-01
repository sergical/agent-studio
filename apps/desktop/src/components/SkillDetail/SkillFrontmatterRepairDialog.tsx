// ============================================================================
// SkillFrontmatterRepairDialog - read-only frontmatter repair preview and authorized
// actions for one exact deployment.
// ============================================================================

import { useState } from "react";
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
  InvocationConflictChoice,
  LifecycleTarget,
} from "@skill-studio/lib";
import { applySkillFrontmatterRepair, previewSkillFrontmatterRepair } from "../../lib/skill-api";
import { diffTheme } from "../../lib/theme";
import { useAppStore } from "../../store/appStore";
import {
  frontmatterRepairCopy,
  INVOCATION_CONFLICT_OPTIONS,
} from "./skill-frontmatter-repair-policy";

interface SkillFrontmatterRepairDialogProps {
  target: LifecycleTarget;
  preview: FrontmatterRepairPreview;
  onClose: () => void;
  onApplied: () => void;
  onEditManually: () => void;
}

export function SkillFrontmatterRepairDialog({
  target,
  preview: initialPreview,
  onClose,
  onApplied,
  onEditManually,
}: SkillFrontmatterRepairDialogProps) {
  const [preview, setPreview] = useState(initialPreview);
  const [applying, setApplying] = useState<FrontmatterRepairApplyMode | null>(null);
  const addToast = useAppStore((state) => state.addToast);
  const theme = diffTheme(useAppStore((state) => state.resolvedTheme));
  const copy = frontmatterRepairCopy(preview.kind);
  const isConflict = preview.kind === "invocation-conflict";
  // A conflict has no single right fix, so nothing can be applied until a side is picked.
  const needsChoice = isConflict && preview.choice === null;
  const choose = (choice: InvocationConflictChoice) => {
    previewSkillFrontmatterRepair(target, preview.kind, choice)
      .then(setPreview)
      .catch((error) =>
        addToast({
          type: "error",
          title: "Couldn't preview this option",
          message: error instanceof Error ? error.message : "Unknown error",
        }),
      );
  };
  const apply = (mode: FrontmatterRepairApplyMode) => {
    setApplying(mode);
    applySkillFrontmatterRepair(target, preview, mode)
      .then(() => {
        addToast({ type: "success", title: copy.success });
        onApplied();
        onClose();
      })
      .catch((error) =>
        addToast({
          type: "error",
          title: copy.failure,
          message: error instanceof Error ? error.message : "Unknown error",
        }),
      )
      .finally(() => setApplying(null));
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="max-w-3xl">
        <DialogHeader>
          <DialogTitle>{copy.dialogTitle}</DialogTitle>
          <DialogDescription>
            {preview.reason} This preview is for {homeRelativePath(preview.path)} ({preview.scope}).
            Nothing changes until you choose an action.
          </DialogDescription>
        </DialogHeader>
        {isConflict && (
          <div className="flex flex-wrap gap-2">
            {INVOCATION_CONFLICT_OPTIONS.map(({ choice, label }) => (
              <Button
                key={choice}
                variant={preview.choice === choice ? "default" : "outline"}
                aria-pressed={preview.choice === choice}
                onClick={() => choose(choice)}
                disabled={applying !== null}
              >
                {label}
              </Button>
            ))}
          </div>
        )}
        <div className="max-h-[55vh] overflow-auto rounded-sm border border-border-subtle">
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
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={applying !== null}>
            Cancel
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
              disabled={applying !== null || needsChoice}
            >
              Fix installed copy
            </Button>
          )}
          {preview.allowed_apply_modes.includes("apply-fix") && (
            <Button onClick={() => apply("apply-fix")} disabled={applying !== null || needsChoice}>
              Apply fix
            </Button>
          )}
          {preview.allowed_apply_modes.includes("fork-and-fix") && (
            <Button
              onClick={() => apply("fork-and-fix")}
              disabled={applying !== null || needsChoice}
            >
              Fork and fix (recommended)
            </Button>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
