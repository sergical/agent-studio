// ============================================================================
// app-shortcuts - The app's global keyboard shortcuts: one definition per
// command (id, display keys, label), consumed by the command palette,
// sidebar tooltips, and the row menu hint, so the hint text and the handler
// in `useAppShortcuts` can never drift apart.
// ============================================================================

export interface ShortcutDef {
  id: string;
  /** Display glyphs, one `Kbd` per entry - e.g. `["⌘", "N"]` or `["/"]`. */
  keys: string[];
  label: string;
}

export const SHORTCUTS = {
  commandPalette: { id: "command-palette", keys: ["⌘", "K"], label: "Command palette" },
  addSkill: { id: "add-skill", keys: ["⌘", "N"], label: "Add skill" },
  settings: { id: "settings", keys: ["⌘", ","], label: "Settings" },
  filterSkills: { id: "filter-skills", keys: ["/"], label: "Filter skills" },
  sync: { id: "sync", keys: [], label: "Sync" },
  toggleTheme: { id: "toggle-theme", keys: [], label: "Toggle theme" },
} satisfies Record<string, ShortcutDef>;

/** `aria-keyshortcuts` value for a shortcut's glyphs - e.g. `["⌘", "N"]` -> `"Meta+N"`. */
export function keyShortcutsFor(shortcut: ShortcutDef): string | undefined {
  if (shortcut.keys.length === 0) return undefined;
  return shortcut.keys.map((key) => (key === "⌘" ? "Meta" : key)).join("+");
}

/** Whether any modal surface (dialog, drawer, popup menu) is currently open - `⌘N`/`⌘,` are
 * blocked while one is, so a shortcut never stacks a second surface on top of it. */
export function isModalSurfaceOpen(): boolean {
  return document.querySelector('[role="dialog"], [role="menu"], [role="listbox"]') !== null;
}
