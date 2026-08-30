// ============================================================================
// PackNamePrompt - Inline "name this pack" modal shown from the selection
// bar's "Create pack" button. Not `window.prompt` - it validates the name
// against the backend's [a-z0-9-]+ rule before submitting.
// ============================================================================

import { useState } from "react";
import { X } from "lucide-react";
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
    <div className="pack-name-prompt-overlay" onClick={onClose}>
      <div className="pack-name-prompt" onClick={(e) => e.stopPropagation()}>
        <div className="pack-name-prompt-header">
          <h3>Create pack</h3>
          <button className="pack-name-prompt-close" onClick={onClose}>
            <X size={18} />
          </button>
        </div>
        <p className="pack-name-prompt-hint">
          Bundles {members.length} skill{members.length !== 1 ? "s" : ""} into{" "}
          <code>~/.agents/packs/{name || "…"}</code>.
        </p>
        <input
          autoFocus
          className="pack-name-prompt-input"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") handleSubmit();
          }}
        />
        {error && <p className="pack-name-prompt-error">{error}</p>}
        <div className="pack-name-prompt-actions">
          <button className="pack-name-prompt-cancel" onClick={onClose}>
            Cancel
          </button>
          <button
            className="pack-name-prompt-submit"
            disabled={!!error || submitting}
            onClick={handleSubmit}
          >
            {submitting ? "Creating…" : "Create"}
          </button>
        </div>
      </div>
    </div>
  );
}
