// ============================================================================
// MaterializeRootDialog - the one place a whole-dir link (e.g.
// `~/.claude/skills -> ../.agents/skills`) gets converted into real per-skill
// links. Opened from the Locations card's Claude Code toggle on a linked
// root, and from Home's "linked-root" repair card - never done silently.
// ============================================================================

import { useState } from "react";
import {
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@skill-studio/ui";
import { homeRelativePath } from "@skill-studio/lib";
import type { LifecycleTarget } from "@skill-studio/lib";
import { materializeHarnessRoot, materializeHarnessRootThenDisable } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";

interface MaterializeRootDialogProps {
  /** The whole-directory-link deployment selected by the caller. */
  target: LifecycleTarget;
  /** The harness's agent id (e.g. `"claude-code"`) - what the backend keys the materialized-root record on. */
  harness: string;
  /** The harness's display label (e.g. "Claude Code"), for the dialog copy only. */
  harnessLabel: string;
  root: string;
  intent: { kind: "convert-only" } | { kind: "convert-then-disable"; skill: string };
  onClose: () => void;
  /** Called once the conversion (and optional disable) succeeds, before `onClose`. */
  onConverted?: () => void;
}

export function MaterializeRootDialog({
  target,
  harness,
  harnessLabel,
  root,
  intent,
  onClose,
  onConverted,
}: MaterializeRootDialogProps) {
  const [isConverting, setIsConverting] = useState(false);
  const addToast = useAppStore((state) => state.addToast);
  const rootLabel = homeRelativePath(root);

  const handleConvert = () => {
    setIsConverting(true);
    const conversion =
      intent.kind === "convert-then-disable"
        ? materializeHarnessRootThenDisable(target, harness, root)
        : materializeHarnessRoot(target, harness, root);
    conversion
      .then(() => {
        onConverted?.();
        onClose();
      })
      .catch((err) => {
        addToast({
          type: "error",
          title:
            intent.kind === "convert-then-disable"
              ? "Couldn't convert and turn off"
              : "Couldn't convert",
          message: err instanceof Error ? err.message : String(err),
        });
      })
      .finally(() => setIsConverting(false));
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Convert {rootLabel} to per-skill links?</DialogTitle>
          <DialogDescription>
            The link at {rootLabel} lets {harnessLabel} read every skill in the Universal folder.
            Converting replaces it with one symlink per skill. The symlinks still point to the
            Universal folder, so no files are copied. You can then switch each skill on or off for{" "}
            {harnessLabel}.
            {intent.kind === "convert-then-disable" && (
              <>
                {" "}
                After conversion, {intent.skill} will be turned off only for {harnessLabel}.
              </>
            )}{" "}
            Activity records this change and provides the undo action.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={isConverting}>
            Cancel
          </Button>
          <Button onClick={handleConvert} disabled={isConverting}>
            {isConverting
              ? "Converting…"
              : intent.kind === "convert-then-disable"
                ? `Convert, then turn off ${intent.skill}`
                : "Convert"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
