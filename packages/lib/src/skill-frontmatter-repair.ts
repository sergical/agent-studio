// ============================================================================
// Skill Studio - frontmatter quote repair
// Pure helper that turns a "mapping values are not allowed" style SKILL.md
// parse error into a one-line fix: wrap the offending plain value in quotes.
// ============================================================================

export interface FrontmatterQuoteRepair {
  key: string;
  /** 1-based, counted from the first line of the file (as the parse error counts). */
  line: number;
  before: string;
  after: string;
  fixedContent: string;
}

export interface YamlFrontmatterErrorLocation {
  line: number;
  column: number;
}

const ERROR_PATTERN = /^invalid YAML frontmatter at line (\d+), column (\d+)/;
const KEY_LINE_PATTERN = /^([A-Za-z0-9_][A-Za-z0-9_.-]*):[ \t]+(\S.*?)[ \t]*$/;
/** Characters that cannot start a YAML plain scalar. */
const INDICATOR_START = /^[*&!|>%@`[{]/;
const FRAGMENT_RADIUS = 20;
const LINE_PREVIEW_LENGTH = 80;

/** Reads the location out of the Rust `invalid YAML frontmatter at line N, column M: ...` message. */
export function parseYamlFrontmatterError(message: string): YamlFrontmatterErrorLocation | null {
  const match = ERROR_PATTERN.exec(message);
  return match ? { line: Number(match[1]), column: Number(match[2]) } : null;
}

function quoteYamlScalar(value: string): string {
  return `"${value.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
}

/** Index of the closing `---` fence, or -1 when the file has no closed frontmatter. */
function closingFenceIndex(lines: string[]): number {
  if (lines[0]?.trim() !== "---") return -1;
  return lines.findIndex((line, index) => index > 0 && line.trim() === "---");
}

/**
 * Proposes wrapping the value on `line` in double quotes. Only the safe common
 * case qualifies: a top-level `key: value` whose single-line plain value holds
 * `: ` or ` #`, or starts with a character YAML reserves. Anything else is `null`.
 *
 * The result is checked without a YAML parser (none is a direct dependency):
 * the new line is `key: "<escaped value>"`, and only `\` and `"` are escaped, so
 * a double-quoted scalar unescapes back to the original value by construction.
 */
export function proposeFrontmatterQuoteRepair(
  content: string,
  line: number,
): FrontmatterQuoteRepair | null {
  // Splitting on a captured separator keeps each line's own ending for the rejoin.
  const parts = content.split(/(\r?\n)/);
  const lines = parts.filter((_, index) => index % 2 === 0);
  const end = closingFenceIndex(lines);
  const index = line - 1;
  if (end < 0 || index < 1 || index >= end) return null;

  const match = KEY_LINE_PATTERN.exec(lines[index]);
  if (!match) return null;
  const [, key, value] = match;
  if (value.startsWith('"') || value.startsWith("'")) return null;
  const needsQuotes = value.includes(": ") || value.includes(" #") || INDICATOR_START.test(value);
  if (!needsQuotes) return null;

  // An indented next line continues this value (block or multi-line plain scalar).
  const next = lines[index + 1];
  if (index + 1 < end && next.trim() !== "" && /^\s/.test(next)) return null;

  const after = `${key}: ${quoteYamlScalar(value)}`;
  const fixedLines = [...lines];
  fixedLines[index] = after;
  const fixedContent = fixedLines.reduce(
    (text, current, at) => text + current + (parts[at * 2 + 1] ?? ""),
    "",
  );
  return { key, line, before: lines[index], after, fixedContent };
}

/** Plain explanation of why the repair's line fails to parse; names the fragment around `column` when known. */
export function describeFrontmatterRepair(repair: FrontmatterQuoteRepair, column?: number): string {
  const value = repair.before.slice(repair.before.indexOf(":") + 1).trim();
  const reason = value.includes(": ")
    ? "has a colon followed by a space, so YAML reads it as a new field"
    : value.includes(" #")
      ? "has a space followed by #, so YAML reads the rest as a comment"
      : `starts with "${value[0]}", which YAML reserves`;
  const at = column !== undefined && column > 0 ? column - 1 : repair.before.indexOf(value);
  const fragment = repair.before
    .slice(Math.max(0, at - FRAGMENT_RADIUS), at + FRAGMENT_RADIUS)
    .trim();
  const shown = fragment.length > 0 ? ` (“${fragment}”)` : "";
  return `The ${repair.key} on line ${repair.line} ${reason}${shown}.`;
}

/** The start of the failing line, so the user knows where to look when no repair exists. */
export function describeFrontmatterErrorLine(content: string, line: number): string | null {
  const text = content.split(/\r?\n/)[line - 1];
  if (text === undefined || text.trim() === "") return null;
  const shown = text.length > LINE_PREVIEW_LENGTH ? `${text.slice(0, LINE_PREVIEW_LENGTH)}…` : text;
  return `SKILL.md has invalid YAML on line ${line}: ${shown}`;
}
