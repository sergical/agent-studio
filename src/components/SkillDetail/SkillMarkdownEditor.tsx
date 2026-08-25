// ============================================================================
// SkillMarkdownEditor - Full-height raw SKILL.md textarea with Save/Cancel
// ============================================================================

import { useEffect, useState } from "react";
import { Save, X } from "lucide-react";

interface SkillMarkdownEditorProps {
  initialContent: string;
  isSaving: boolean;
  onSave: (content: string) => void;
  onCancel: () => void;
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
}: SkillMarkdownEditorProps) {
  const [content, setContent] = useState(initialContent);
  const isDirty = content !== initialContent;

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === "s") {
        e.preventDefault();
        if (isSaving || !isDirty) return;
        onSave(content);
      }
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [content, onSave, isSaving, isDirty]);

  return (
    <div className="skill-markdown-editor">
      <div className="skill-markdown-editor-toolbar">
        {isDirty && <span className="skill-markdown-editor-dirty">Unsaved changes</span>}
        <div className="skill-markdown-editor-actions">
          <button className="skill-action-button" onClick={onCancel} disabled={isSaving}>
            <X size={14} />
            Cancel
          </button>
          <button
            className="skill-action-button primary"
            onClick={() => onSave(content)}
            disabled={isSaving || !isDirty}
          >
            <Save size={14} />
            {isSaving ? "Saving…" : "Save"}
          </button>
        </div>
      </div>
      <textarea
        className="skill-markdown-editor-textarea"
        value={content}
        onChange={(e) => setContent(e.target.value)}
        spellCheck={false}
      />
    </div>
  );
}
