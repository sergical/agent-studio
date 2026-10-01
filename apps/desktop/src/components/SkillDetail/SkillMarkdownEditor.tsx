// ============================================================================
// SkillMarkdownEditor - Raw SKILL.md textarea with a line-number gutter and Save/Cancel
// ============================================================================

import { useEffect, useEffectEvent, useRef, useState } from "react";
import { Save, X } from "lucide-react";
import { Button, Textarea } from "@skill-studio/ui";
import { DiscardChangesDialog } from "./DiscardChangesDialog";
import { lineRange } from "./skill-editor-lines";

interface GutterLayout {
  /** Rendered height of each logical line, wrapped rows included. */
  heights: number[];
  fontSize: string;
  lineHeight: string;
  paddingTop: string;
}

/**
 * Measures how tall each logical line renders in `textarea` by laying the same
 * text out in a hidden mirror with the textarea's width, font, padding and
 * wrapping, so a wrapped line's number still sits at its first row.
 */
function measureGutterLayout(textarea: HTMLTextAreaElement, content: string): GutterLayout {
  const style = getComputedStyle(textarea);
  const mirror = document.createElement("div");
  Object.assign(mirror.style, {
    position: "absolute",
    visibility: "hidden",
    top: "0",
    left: "-9999px",
    boxSizing: "border-box",
    width: `${textarea.clientWidth}px`,
    border: "0",
    padding: `0 ${style.paddingRight} 0 ${style.paddingLeft}`,
    font: style.font,
    letterSpacing: style.letterSpacing,
    tabSize: style.tabSize,
    whiteSpace: style.whiteSpace,
    wordBreak: style.wordBreak,
    overflowWrap: style.overflowWrap,
  });
  for (const line of content.split("\n")) {
    const row = document.createElement("div");
    // An empty line collapses to zero height; a zero-width space keeps one row.
    row.textContent = line === "" ? "\u200b" : line;
    mirror.appendChild(row);
  }
  document.body.appendChild(mirror);
  const heights = Array.from(mirror.children, (row) => row.getBoundingClientRect().height);
  mirror.remove();
  return {
    heights,
    fontSize: style.fontSize,
    lineHeight: style.lineHeight,
    paddingTop: `calc(${style.paddingTop} + ${style.borderTopWidth})`,
  };
}

function sameLayout(a: GutterLayout | null, b: GutterLayout): boolean {
  return (
    a !== null &&
    a.fontSize === b.fontSize &&
    a.lineHeight === b.lineHeight &&
    a.paddingTop === b.paddingTop &&
    a.heights.length === b.heights.length &&
    a.heights.every((height, i) => height === b.heights[i])
  );
}

interface SkillMarkdownEditorProps {
  initialContent: string;
  isSaving: boolean;
  onSave: (content: string) => void;
  onCancel: () => void;
  /** Notified on every dirty-state change, so the page-level Escape handler knows whether leaving needs confirmation. */
  onDirtyChange?: (isDirty: boolean) => void;
  /** Label for the Save button while not saving - "Save" unless the caller overrides it (e.g. "Fork and save"). */
  saveLabel?: string;
  /** 1-based SKILL.md line to mark in the gutter and select on open, e.g. the line of a YAML error. */
  highlightLine?: number;
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
  highlightLine,
}: SkillMarkdownEditorProps) {
  const [content, setContent] = useState(initialContent);
  const isDirty = content !== initialContent;
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const gutterRef = useRef<HTMLDivElement>(null);
  const [showDiscardDialog, setShowDiscardDialog] = useState(false);
  const [layout, setLayout] = useState<GutterLayout | null>(null);
  const lineCount = content.split("\n").length;

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

  // Re-measured on every content change and whenever the textarea's width
  // changes (window or drag-resize), since either can re-wrap lines.
  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    const measure = () => {
      const next = measureGutterLayout(textarea, content);
      setLayout((prev) => (sameLayout(prev, next) ? prev : next));
    };
    const observer = new ResizeObserver(measure);
    observer.observe(textarea);
    return () => observer.disconnect();
  }, [content]);

  // Opens on the requested line: selects it and scrolls it into the viewport.
  // Runs only when the target changes, so typing never yanks the view back.
  useEffect(() => {
    const textarea = textareaRef.current;
    const range = highlightLine === undefined ? null : lineRange(initialContent, highlightLine);
    if (!textarea || !range || highlightLine === undefined) return;
    const { heights, paddingTop } = measureGutterLayout(textarea, initialContent);
    const lineTop = heights.slice(0, highlightLine - 1).reduce((sum, height) => sum + height, 0);
    textarea.focus();
    textarea.setSelectionRange(range.start, range.end);
    textarea.scrollTop = Math.max(0, lineTop + Number.parseFloat(paddingTop) - 40);
  }, [highlightLine, initialContent]);

  return (
    <div className="select-text flex flex-col gap-2 p-4">
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
      <div className="flex rounded-sm border border-border bg-bg-primary focus-within:border-border-focus">
        <div
          ref={gutterRef}
          aria-hidden="true"
          className="shrink-0 select-none overflow-hidden border-r border-border-subtle px-2 text-right font-mono text-body leading-[1.5] tabular-nums text-text-tertiary"
          style={
            layout
              ? {
                  fontSize: layout.fontSize,
                  lineHeight: layout.lineHeight,
                  paddingTop: layout.paddingTop,
                }
              : { paddingTop: "0.625rem" }
          }
        >
          {Array.from({ length: lineCount }, (_, i) => (
            <div
              key={i}
              style={{ height: layout?.heights[i] }}
              className={i + 1 === highlightLine ? "font-semibold text-warning" : undefined}
            >
              {i + 1}
            </div>
          ))}
        </div>
        <Textarea
          ref={textareaRef}
          className="max-h-[75vh] min-h-[60vh] resize-y rounded-none border-0 bg-transparent px-3 py-2.5 font-mono text-body leading-[1.5] text-text-primary focus-visible:ring-0"
          value={content}
          onChange={(e) => handleContentChange(e.target.value)}
          onScroll={(e) => {
            if (gutterRef.current) gutterRef.current.scrollTop = e.currentTarget.scrollTop;
          }}
          spellCheck={false}
        />
      </div>
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
