// ============================================================================
// MakeIndependentCopyDialog - confirms detaching one exact linked deployment.
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
import type { Deployment } from "@skill-studio/lib";
import { makeSkillIndependentCopy } from "../../lib/skill-api";
import { useAppStore } from "../../store/appStore";

interface MakeIndependentCopyDialogProps {
  skillName: string;
  deployment: Deployment;
  scopeLabel: string;
  onClose: () => void;
}

/** Confirms and runs a copy operation for one selected linked deployment. */
export function MakeIndependentCopyDialog({
  skillName,
  deployment,
  scopeLabel,
  onClose,
}: MakeIndependentCopyDialogProps) {
  const [isCopying, setIsCopying] = useState(false);
  const addToast = useAppStore((state) => state.addToast);

  const handleCopy = () => {
    setIsCopying(true);
    makeSkillIndependentCopy({ deployment_id: deployment.id })
      .then(onClose)
      .catch((error) => {
        addToast({
          type: "error",
          title: "Couldn't make independent copy",
          message: error instanceof Error ? error.message : "Unknown error",
        });
      })
      .finally(() => setIsCopying(false));
  };

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Make independent copy of {skillName}?</DialogTitle>
          <DialogDescription>
            This replaces the {deployment.agent} link at {homeRelativePath(deployment.path)} with a
            local copy in {scopeLabel}. Universal updates will no longer change this copy. The skill
            stays on for {deployment.agent}.
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={onClose} disabled={isCopying}>
            Cancel
          </Button>
          <Button onClick={handleCopy} disabled={isCopying}>
            {isCopying ? "Copying…" : "Make independent copy"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
