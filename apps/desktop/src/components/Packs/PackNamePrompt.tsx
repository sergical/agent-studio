// ============================================================================
// PackNamePrompt - Inline "name this pack" modal shown from the selection
// bar's "Create pack" button. Not `window.prompt` - it validates the name
// against the backend's [a-z0-9-]+ rule before submitting.
// ============================================================================

import { useState } from "react";
import {
  Button,
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Input,
} from "@skill-studio/ui";
import { createSkillPack } from "../../lib/skill-api";
import { DEFAULT_PACK_NAME, validatePackName } from "@skill-studio/lib";
import type { PackMember } from "@skill-studio/lib";
import { useAppStore } from "../../store/appStore";

interface PackNamePromptProps {
  members: PackMember[];
  onClose: () => void;
  onCreated: () => void;
}

export function PackNamePrompt({ members, onClose, onCreated }: PackNamePromptProps) {
  const [name, setName] = useState(DEFAULT_PACK_NAME);
  const [submitting, setSubmitting] = useState(false);
  const addToast = useAppStore((state) => state.addToast);
  const error = validatePackName(name);

  // A Promise chain, not a try/finally statement, so the compiler can still
  // optimize this component (it doesn't support `finally` clauses yet).
  function handleSubmit() {
    if (error || submitting) return;
    setSubmitting(true);
    return createSkillPack(name, members)
      .then(() => {
        addToast({
          type: "success",
          title: "Pack created",
          message: `${name} · ${members.length} skill${members.length !== 1 ? "s" : ""}`,
        });
        onCreated();
      })
      .catch((err) => {
        addToast({
          type: "error",
          title: "Couldn't create pack",
          message: err instanceof Error ? err.message : String(err),
        });
      })
      .finally(() => {
        setSubmitting(false);
      });
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="w-95 max-w-[calc(100%-2rem)]" aria-label="Create pack">
        <DialogHeader>
          <DialogTitle>Create pack</DialogTitle>
        </DialogHeader>
        <p className="m-0 text-wrap-pretty text-small text-text-tertiary">
          Bundles {members.length} skill{members.length !== 1 ? "s" : ""} into{" "}
          <code>~/.agents/packs/{name || "…"}</code>.
        </p>
        <Input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") handleSubmit();
          }}
        />
        {error && <p className="m-0 mt-1.5 text-small text-error">{error}</p>}
        <DialogFooter>
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button disabled={!!error || submitting} onClick={handleSubmit}>
            {submitting ? "Creating…" : "Create"}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
