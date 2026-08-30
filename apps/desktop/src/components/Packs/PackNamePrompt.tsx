// ============================================================================
// PackNamePrompt - Inline "name this pack" modal shown from the selection
// bar's "Create pack" button. Not `window.prompt` - it validates the name
// against the backend's [a-z0-9-]+ rule before submitting.
// ============================================================================

import { useState } from "react";
import { X } from "lucide-react";
import { Button, Input } from "@skill-studio/ui";
import { createSkillPack } from "../../lib/skill-api";
import { DEFAULT_PACK_NAME, validatePackName } from "../../lib/skill-pack-name";
import type { PackMember } from "../../lib/skill-types";
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

  async function handleSubmit() {
    if (error || submitting) return;
    setSubmitting(true);
    try {
      await createSkillPack(name, members);
      addToast({
        type: "success",
        title: "Pack created",
        message: `${name} · ${members.length} skill${members.length !== 1 ? "s" : ""}`,
      });
      onCreated();
    } catch (err) {
      addToast({
        type: "error",
        title: "Couldn't create pack",
        message: err instanceof Error ? err.message : String(err),
      });
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-modal flex items-center justify-center bg-scrim"
      onClick={onClose}
    >
      <div
        className="w-95 rounded-lg border border-border bg-bg-elevated p-5 shadow-lg"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="mb-2 flex items-center justify-between">
          <h3 className="m-0 text-wrap-balance text-emphasis font-semibold text-text-primary">
            Create pack
          </h3>
          <button
            className="flex size-6 items-center justify-center rounded-sm border-0 bg-transparent text-text-tertiary"
            onClick={onClose}
          >
            <X size={18} />
          </button>
        </div>
        <p className="m-0 mb-3 text-wrap-pretty text-small text-text-tertiary">
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
        <div className="mt-4 flex justify-end gap-2">
          <Button variant="outline" onClick={onClose}>
            Cancel
          </Button>
          <Button disabled={!!error || submitting} onClick={handleSubmit}>
            {submitting ? "Creating…" : "Create"}
          </Button>
        </div>
      </div>
    </div>
  );
}
