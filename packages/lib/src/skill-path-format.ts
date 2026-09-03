// ============================================================================
// Skill Studio - skill-path-format
// Two ways of shortening an absolute path for display: `homeRelativePath`
// swaps a leading home directory for "~", `shortProjectPath` keeps only the
// last two segments. Moved out of SkillListFilterBar.tsx so SkillLocationsCard
// can share the same logic.
// ============================================================================

/** `/Users/x/...` or `/home/x/...` -> `/Users/x` or `/home/x`, `null` for anything else. */
function guessHomeDir(path: string): string | null {
  const match = /^(\/(?:Users|home)\/[^/]+)/.exec(path);
  return match ? match[1] : null;
}

/**
 * `/Users/x/src/a` -> `~/src/a`. `home` overrides the guessed home directory
 * (useful for tests, or once the real home dir is known); without it, the
 * home directory is guessed from the path's own `/Users/<user>` or
 * `/home/<user>` prefix. Paths outside any home directory are returned as-is.
 */
export function homeRelativePath(path: string, home?: string): string {
  const base = home ?? guessHomeDir(path);
  if (!base || !path.startsWith(base)) return path;
  return `~${path.slice(base.length)}`;
}

/** A shortened "~/…/last-two-segments" form of `path`, e.g. for a project menu's secondary line. */
export function shortProjectPath(path: string): string {
  const segments = path.split("/").filter(Boolean);
  return `~/${segments.slice(-2).join("/")}`;
}

/**
 * Everything before a path's last segment. Used wherever a deployment's
 * skills root has to be derived from the skill directory it points at.
 */
export function parentDirectory(path: string): string {
  const idx = path.lastIndexOf("/");
  return idx > 0 ? path.slice(0, idx) : path;
}
