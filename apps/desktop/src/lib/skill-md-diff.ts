// ============================================================================
// Skill Studio - skill-md-diff
// Splits a SKILL.md rewrite into per-hunk unified diffs the user can accept
// or reject individually, and reassembles the accepted ones into a patch
// ============================================================================

import { applyPatch, FILE_HEADERS_ONLY, formatPatch, structuredPatch } from "diff";
import type { StructuredPatch, StructuredPatchHunk } from "diff";

/** Lines of context kept around each change, matching a typical `git diff`. */
const CONTEXT_LINES = 3;

/** One hunk of a SKILL.md rewrite, independently acceptable or rejectable. */
export interface SkillMdHunk {
  index: number;
  header: string;
  /** A complete unified diff containing only this hunk, for `PatchDiff` to render. */
  patchText: string;
  /** The same hunk in structured form, so applying it doesn't need to reparse `patchText`. */
  hunk: StructuredPatchHunk;
  accepted: boolean;
}

/** Formats one hunk as a standalone unified diff against `SKILL.md`. */
function formatHunk(hunk: StructuredPatchHunk): string {
  const patch: StructuredPatch = {
    oldFileName: "a/SKILL.md",
    newFileName: "b/SKILL.md",
    oldHeader: undefined,
    newHeader: undefined,
    hunks: [hunk],
  };
  return formatPatch(patch, FILE_HEADERS_ONLY);
}

/**
 * Diffs `oldText` against `newText` and returns one `SkillMdHunk` per hunk,
 * all `accepted` by default. Identical texts produce no hunks; a trailing
 * newline difference is still a real hunk.
 */
export function diffSkillMd(oldText: string, newText: string): SkillMdHunk[] {
  const patch = structuredPatch("SKILL.md", "SKILL.md", oldText, newText, undefined, undefined, {
    context: CONTEXT_LINES,
  });
  return patch.hunks.map((hunk, index) => ({
    index,
    header: `@@ -${hunk.oldStart},${hunk.oldLines} +${hunk.newStart},${hunk.newLines} @@`,
    patchText: formatHunk(hunk),
    hunk,
    accepted: true,
  }));
}

/**
 * A single read-only unified diff between `oldText` and `newText`, for
 * `SkillCompareDialog` - unlike `diffSkillMd`, this isn't split into
 * individually-acceptable hunks, since both sides are already-saved content,
 * not a proposal to accept or reject.
 */
export function unifiedSkillMdDiff(oldText: string, newText: string): string {
  const patch = structuredPatch("SKILL.md", "SKILL.md", oldText, newText, undefined, undefined, {
    context: CONTEXT_LINES,
  });
  return formatPatch(
    { ...patch, oldFileName: "a/SKILL.md", newFileName: "b/SKILL.md" },
    FILE_HEADERS_ONLY,
  );
}

/**
 * Applies only the accepted hunks to `oldText`, in order. Returns null when
 * jsdiff can't fit them - e.g. `oldText` no longer matches the context the
 * hunks were computed against.
 */
export function applyAcceptedHunks(oldText: string, hunks: SkillMdHunk[]): string | null {
  const accepted = hunks.filter((hunk) => hunk.accepted);
  if (accepted.length === 0) return oldText;
  const patch: StructuredPatch = {
    oldFileName: "a/SKILL.md",
    newFileName: "b/SKILL.md",
    oldHeader: undefined,
    newHeader: undefined,
    hunks: accepted.map((accepted) => accepted.hunk),
  };
  const result = applyPatch(oldText, patch);
  return result === false ? null : result;
}
