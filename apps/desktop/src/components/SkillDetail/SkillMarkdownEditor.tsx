// ============================================================================
// SkillMarkdownEditor - Full-height raw SKILL.md textarea with Save/Cancel
// ============================================================================

import { useEffect, useEffectEvent, useRef, useState } from "react";
import { Save, X } from "lucide-react";
import { Button, Textarea } from "@skill-studio/ui";
import { DiscardChangesDialog } from "./DiscardChangesDialog";

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
  const [showDiscardDialog, setShowDiscardDialog] = useState(false);

  // Notified from the change handler itself, not an effect syncing a derived
  // value up to the parent - it only needs to fire on an actual dirty-state
  // flip, same as the effect it replaces.
  const handleContentChange = (value: string) => {
    const nextDirty = value !== initialContent;
    setContent(value);
    if (nextDirty !== isDirty) onDirtyChange?.(nextDirty);
  };

  const handleCancel = () => {
    if (isDirty) {
      setShowDiscardDialog(true);
      return;
    }
    onCancel();
  };

  // Reads the latest content/isSaving/isDirty/onSave/onCancel without
  // re-subscribing the listener on every keystroke.
  const onKeyboardShortcut = useEffectEvent((e: KeyboardEvent) => {
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
  });

  useEffect(() => {
    function handleKeyDown(e: KeyboardEvent) {
      onKeyboardShortcut(e);
    }
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);

  return (
    <div className="flex flex-col gap-2 p-4">
      <div className="flex items-center justify-end gap-3">
        {isDirty && <span className="mr-auto text-caption text-warning">Unsaved changes</span>}
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={handleCancel} disabled={isSaving}>
            <X size={14} />
            Cancel
          </Button>
          <Button size="sm" onClick={() => onSave(content)} disabled={isSaving || !isDirty}>
            <Save size={14} />
            {isSaving ? "Saving…" : saveLabel}
          </Button>
        </div>
      </div>
      <Textarea
        ref={textareaRef}
        className="min-h-[60vh] resize-y rounded-sm border-border bg-bg-primary px-3 py-2.5 font-mono text-body text-text-primary focus-visible:border-border-focus focus-visible:ring-0"
        value={content}
        onChange={(e) => handleContentChange(e.target.value)}
        spellCheck={false}
      />
      <DiscardChangesDialog
        open={showDiscardDialog}
        onOpenChange={setShowDiscardDialog}
        onDiscard={() => {
          setShowDiscardDialog(false);
          onCancel();
        }}
      />
    </div>
  );
}
