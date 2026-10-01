// ============================================================================
// SkillMarkdownEditor - Raw SKILL.md textarea with a line-number gutter and Save/Cancel
// ============================================================================

import { useEffect, useEffectEvent, useLayoutEffect, useRef, useState } from "react";
import { Save, X } from "lucide-react";
import { Button, Textarea } from "@skill-studio/ui";
import { DiscardChangesDialog } from "./DiscardChangesDialog";
import {
  contentForSave,
  isContentDirty,
  lineRange,
  normalizeLineEndings,
} from "./skill-editor-lines";

interface GutterLayout {
  /** Rendered height of each logical line, wrapped rows included. */
  heights: number[];
  fontSize: string;
  lineHeight: string;
  /** Textarea padding plus border on each side: where its first and last rows sit inside the scroll area. */
  paddingTopPx: number;
  paddingBottomPx: number;
}

/**
 * Measures how tall each logical line renders in `textarea` by laying the same
 * text out in a hidden mirror with the textarea's width, font, padding and
 * wrapping, so a wrapped line's number still sits at its first row.
 */
function measureGutterLayout(textarea: HTMLTextAreaElement, content: string): GutterLayout {
  const style = getComputedStyle(textarea);
  const borderX =
    Number.parseFloat(style.borderLeftWidth) + Number.parseFloat(style.borderRightWidth);
  // Whatever of offsetWidth is neither border nor content is the vertical scrollbar.
  const scrollbarWidth = textarea.offsetWidth - textarea.clientWidth - borderX;
  const mirror = document.createElement("div");
  Object.assign(mirror.style, {
    position: "absolute",
    visibility: "hidden",
    top: "0",
    left: "-9999px",
    boxSizing: "border-box",
    width: `${textarea.getBoundingClientRect().width - borderX - scrollbarWidth}px`,
    border: "0",
    padding: `0 ${style.paddingRight} 0 ${style.paddingLeft}`,
    // Longhands, because some engines report the `font` shorthand as "".
    fontFamily: style.fontFamily,
    fontSize: style.fontSize,
    fontStyle: style.fontStyle,
    fontWeight: style.fontWeight,
    lineHeight: style.lineHeight,
    letterSpacing: style.letterSpacing,
    tabSize: style.tabSize,
    whiteSpace: style.whiteSpace,
    wordBreak: style.wordBreak,
    overflowWrap: style.overflowWrap,
  });
  for (const line of normalizeLineEndings(content).split("\n")) {
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
    paddingTopPx: Number.parseFloat(style.paddingTop) + Number.parseFloat(style.borderTopWidth),
    paddingBottomPx:
      Number.parseFloat(style.paddingBottom) + Number.parseFloat(style.borderBottomWidth),
  };
}

function applyGutterLayout(
  textarea: HTMLTextAreaElement,
  content: string,
  setLayout: (update: (prev: GutterLayout | null) => GutterLayout | null) => void,
) {
  const next = measureGutterLayout(textarea, content);
  setLayout((prev) => (sameLayout(prev, next) ? prev : next));
}

function sameLayout(a: GutterLayout | null, b: GutterLayout): boolean {
  return (
    a !== null &&
    a.fontSize === b.fontSize &&
    a.lineHeight === b.lineHeight &&
    a.paddingTopPx === b.paddingTopPx &&
    a.paddingBottomPx === b.paddingBottomPx &&
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
  const [content, setContent] = useState(() => normalizeLineEndings(initialContent));
  const isDirty = isContentDirty(content, initialContent);
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const gutterRef = useRef<HTMLDivElement>(null);
  const [showDiscardDialog, setShowDiscardDialog] = useState(false);
  const [layout, setLayout] = useState<GutterLayout | null>(null);
  const lineCount = content.split("\n").length;
  const markedLine = isDirty ? undefined : highlightLine;

  // Notified from the change handler itself, not an effect syncing a derived
  // value up to the parent - it only needs to fire on an actual dirty-state
  // flip, same as the effect it replaces.
  const handleContentChange = (value: string) => {
    const nextDirty = isContentDirty(value, initialContent);
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
      onSave(contentForSave(content, initialContent));
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

  const measureLatest = useEffectEvent(() => {
    const textarea = textareaRef.current;
    if (textarea) applyGutterLayout(textarea, content, setLayout);
  });

  // Before paint, so line numbers never trail an edit by a frame.
  useLayoutEffect(() => {
    const textarea = textareaRef.current;
    if (textarea) applyGutterLayout(textarea, content, setLayout);
  }, [content]);

  // Width changes (window or drag-resize) can re-wrap lines without a content change.
  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea) return;
    let frame = 0;
    // One measure per frame: a drag-resize fires the observer many times per frame.
    const observer = new ResizeObserver(() => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => measureLatest());
    });
    observer.observe(textarea);
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
    };
  }, []);

  // Every render: a new layout or line count changes the gutter's height and
  // padding, which can leave its scroll offset behind the textarea's until the
  // next scroll event. Assigning an unchanged scrollTop is a no-op.
  useLayoutEffect(() => {
    if (gutterRef.current && textareaRef.current) {
      gutterRef.current.scrollTop = textareaRef.current.scrollTop;
    }
  });

  // Opens on the requested line: selects it and scrolls it into the viewport.
  // Runs only when the target changes, so typing never yanks the view back.
  useEffect(() => {
    const textarea = textareaRef.current;
    const range = highlightLine === undefined ? null : lineRange(initialContent, highlightLine);
    if (!textarea || !range || highlightLine === undefined) return;
    const { heights, paddingTopPx } = measureGutterLayout(textarea, initialContent);
    const lineTop = heights.slice(0, highlightLine - 1).reduce((sum, height) => sum + height, 0);
    textarea.focus();
    textarea.setSelectionRange(range.start, range.end);
    textarea.scrollTop = Math.max(0, lineTop + paddingTopPx - 40);
  }, [highlightLine, initialContent]);

  // `contain: size` stops the gutter's full-length content from stretching the
  // row, so its width can't come from content: px-2 (1rem) + border-r (1px) + digits.
  const gutterWidth = `calc(${String(lineCount).length}ch + 1rem + 1px)`;

  return (
    <div className="select-text flex flex-col gap-2 p-4">
      <div className="flex items-center justify-end gap-3">
        {isDirty && <span className="mr-auto text-caption text-warning">Unsaved changes</span>}
        <div className="flex gap-2">
          <Button variant="outline" size="sm" onClick={handleCancel} disabled={isSaving}>
            <X size={14} />
            Cancel
          </Button>
          <Button
            size="sm"
            onClick={() => onSave(contentForSave(content, initialContent))}
            disabled={isSaving || !isDirty}
          >
            <Save size={14} />
            {isSaving ? "Saving…" : saveLabel}
          </Button>
        </div>
      </div>
      <div className="flex rounded-sm border border-border bg-bg-primary focus-within:border-border-focus">
        <div
          ref={gutterRef}
          aria-hidden="true"
          className="shrink-0 select-none overflow-hidden [contain:size] border-r border-border-subtle px-2 text-right font-mono text-body leading-[1.5] tabular-nums text-text-tertiary"
          style={
            layout
              ? {
                  fontSize: layout.fontSize,
                  lineHeight: layout.lineHeight,
                  paddingTop: layout.paddingTopPx,
                  paddingBottom: layout.paddingBottomPx,
                  width: gutterWidth,
                }
              : { paddingTop: "0.625rem", paddingBottom: "0.625rem", width: gutterWidth }
          }
        >
          {Array.from({ length: lineCount }, (_, i) => (
            <div
              key={i}
              style={{ height: layout?.heights[i] }}
              className={i + 1 === markedLine ? "font-semibold text-warning" : undefined}
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
