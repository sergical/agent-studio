// ============================================================================
// Skill Studio - Theme resolution
// The one place that turns a stored theme preference plus the OS's
// prefers-color-scheme into the "light" | "dark" that actually gets painted.
// Used both by the pre-paint stamp in main.tsx (before React renders, so
// there's no first-frame flash) and by the store's `setTheme` - sharing this
// module is what keeps the two from disagreeing.
// ============================================================================

export type Theme = "system" | "light" | "dark";
export type ResolvedTheme = "light" | "dark";

export const THEME_STORAGE_KEY = "theme";

/** True when the OS reports a dark color scheme preference; false outside a browser (e.g. tests). */
export function systemPrefersDark(): boolean {
  return globalThis.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
}

/** Pure: a stored preference plus the OS preference in, "light" | "dark" out. */
export function resolveTheme(theme: Theme, prefersDark: boolean): ResolvedTheme {
  if (theme === "light" || theme === "dark") return theme;
  return prefersDark ? "dark" : "light";
}

/** The user's stored theme preference, defaulting to "system" when unset or unreadable. */
export function loadStoredTheme(): Theme {
  try {
    const stored = localStorage.getItem(THEME_STORAGE_KEY);
    return stored === "light" || stored === "dark" || stored === "system" ? stored : "system";
  } catch {
    return "system";
  }
}

/**
 * Stamps `data-theme` on `<html>` before React ever renders, so the app
 * never flashes the wrong palette on first paint. Call once, at the top of
 * main.tsx, before `createRoot`.
 */
export function stampInitialTheme(): void {
  document.documentElement.setAttribute(
    "data-theme",
    resolveTheme(loadStoredTheme(), systemPrefersDark()),
  );
}

/**
 * Applies `resolved` to `<html>`, freezing every transition for one frame
 * first so a theme switch doesn't turn each component's own transition
 * timing into a ragged cascade. A no-op when `resolved` is already painted
 * (the common case: called once at store init, right after the pre-paint
 * stamp already set it).
 */
export function stampTheme(resolved: ResolvedTheme): void {
  const root = globalThis.document?.documentElement;
  if (!root || root.getAttribute("data-theme") === resolved) return;
  root.classList.add("theme-switching");
  root.setAttribute("data-theme", resolved);
  requestAnimationFrame(() => root.classList.remove("theme-switching"));
}

/** Listens for OS theme changes; returns a cleanup that removes the listener. */
export function watchSystemTheme(onChange: (prefersDark: boolean) => void): () => void {
  const mql = globalThis.matchMedia?.("(prefers-color-scheme: dark)");
  if (!mql) return () => {};
  const listener = () => onChange(mql.matches);
  mql.addEventListener("change", listener);
  return () => mql.removeEventListener("change", listener);
}

/**
 * `PatchDiff`'s own theme name for `resolved` - shared by `SkillRunDiff`,
 * `SkillProposedEdits`, and `SkillCompareDialog`, the three `PatchDiff`
 * callers. Takes the store's `resolvedTheme` (not a fresh DOM read) so those
 * components re-render on a theme change instead of only picking it up on
 * their next unrelated render.
 */
export function diffTheme(resolved: ResolvedTheme): "github-light" | "github-dark" {
  return resolved === "light" ? "github-light" : "github-dark";
}
