// ============================================================================
// skill-editor-lines - pure line helpers for SkillMarkdownEditor's gutter
// ============================================================================

/**
 * Character range of the 1-based `line` in `content`, newline excluded, or
 * `null` when the text has no such line. Used to select a line in the textarea.
 */
export function lineRange(content: string, line: number): { start: number; end: number } | null {
  if (!Number.isInteger(line) || line < 1) return null;
  let start = 0;
  for (let current = 1; current < line; current += 1) {
    const newline = content.indexOf("\n", start);
    if (newline === -1) return null;
    start = newline + 1;
  }
  const newline = content.indexOf("\n", start);
  return { start, end: newline === -1 ? content.length : newline };
}
