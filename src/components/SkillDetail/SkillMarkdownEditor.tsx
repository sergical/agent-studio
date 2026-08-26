// ============================================================================
// SkillMarkdownEditor - Full-height raw SKILL.md textarea with Save/Cancel
// ============================================================================

import { useEffect, useRef, useState } from "react";
import { Save, X } from "lucide-react";

interface SkillMarkdownEditorProps {
  initialContent: string;
  isSaving: boolean;
  onSave: (content: string) => void;
  onCancel: () => void;
  /** Notified on every dirty-state change, so the page-level Escape handler knows whether leaving needs confirmation. */
  onDirtyChange?: (isDirty: boolean) => void;
  /** Label for the Save button while not saving - "Save" unless the caller overrides it (e.g. "Fork and save"). */
  saveLabel?: string;
}

/**
 * Raw-text editor for a skill's `SKILL.md` (frontmatter included). Cmd+S and
 * the Save button both submit; the dirty indicator tracks unsaved edits so a
 * stray Cancel click doesn't silently drop them.
 */
export function SkillMarkdownEditor({
  initialContent,
  isSaving,
  onSave,
  onCancel,
  onDirtyChange,
  saveLabel = "Save",
}: SkillMarkdownEditorProps) {
  const [content, setContent] = useState(initialContent);
  const isDirty = content !== initialContent;
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    onDirtyChange?.(isDirty);
  }, [isDirty, onDirtyChange]);

  const handleCancel = () => {
    if (isDirty && !window.confirm("Discard unsaved changes?")) return;
    onCancel();
  };

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "s") {
        e.preventDefault();
        if (isSaving || !isDirty) return;
        onSave(content);
        return;
      }
      if (e.key === "Escape") {
        // Only Escape typed into this editor's own textarea cancels editing -
        // Escape from any other input, contenteditable region, or an open
        // dialog belongs to that widget, not to this editor.
        if (e.target !== textareaRef.current) return;
        if (isDirty) return; // Cancel button's own confirm is the only way out of dirty edits via Escape.
        onCancel();
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [content, onSave, isSaving, isDirty, onCancel]);

  return (
    <div className="skill-markdown-editor">
      <div className="skill-markdown-editor-toolbar">
        {isDirty && <span className="skill-markdown-editor-dirty">Unsaved changes</span>}
        <div className="skill-markdown-editor-actions">
          <button className="skill-action-button" onClick={handleCancel} disabled={isSaving}>
            <X size={14} />
            Cancel
          </button>
          <button
            className="skill-action-button primary"
            onClick={() => onSave(content)}
            disabled={isSaving || !isDirty}
          >
            <Save size={14} />
            {isSaving ? "Saving…" : saveLabel}
          </button>
        </div>
      </div>
      <textarea
        ref={textareaRef}
        className="skill-markdown-editor-textarea"
        value={content}
        onChange={(e) => setContent(e.target.value)}
        spellCheck={false}
      />
    </div>
  );
}
