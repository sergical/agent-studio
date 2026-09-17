// ============================================================================
// command-palette-items - Builds and ranks the palette's flat item list:
// Actions, Go to (places), and every installed skill. No fuzzy-match
// library - matching is case-insensitive substring/word-start/prefix, the
// same tier order a user's own typing intuition expects.
// ============================================================================

import type { ReactNode } from "react";
import type { ActiveView } from "../../store/appStore";
import type { ShortcutDef } from "../../lib/app-shortcuts";
import type { InstalledSkill } from "@skill-studio/lib";

export type PaletteSection = "actions" | "goto" | "skills";

export interface PaletteItem {
  id: string;
  section: PaletteSection;
  label: string;
  /** Secondary text under the label - a skill's state group ("Needs attention", "Healthy", "Parked"). */
  secondary?: string;
  icon?: ReactNode;
  shortcut?: ShortcutDef;
  run: () => void;
}

export const SECTION_LABEL = {
  actions: "Actions",
  goto: "Go to",
  skills: "Skills",
} satisfies Record<PaletteSection, string>;

/** Skills shown for an empty query - enough to browse without overwhelming the popup. */
const EMPTY_QUERY_SKILL_COUNT = 8;

/** One item's match tier for a query - lower sorts first. `undefined` means no match. */
function matchTier(label: string, query: string): number | undefined {
  const haystack = label.toLowerCase();
  if (haystack.startsWith(query)) return 0;
  const wordStart = haystack.split(/[\s/_-]+/).some((word) => word.startsWith(query));
  if (wordStart) return 1;
  if (haystack.includes(query)) return 2;
  return undefined;
}

/** Every Skills-view item, in ranked order, for a query. Ties break alphabetically, then a
 * stable index so equal-ranked items don't reorder between keystrokes. */
export function rankItems(items: PaletteItem[], query: string): PaletteItem[] {
  const trimmed = query.trim().toLowerCase();
  if (trimmed === "") {
    const actions = items.filter((item) => item.section === "actions");
    const goto = items.filter((item) => item.section === "goto");
    const skills = items
      .filter((item) => item.section === "skills")
      .slice(0, EMPTY_QUERY_SKILL_COUNT);
    return [...actions, ...goto, ...skills];
  }
  return items
    .map((item, index) => ({ item, index, tier: matchTier(item.label, trimmed) }))
    .filter(
      (entry): entry is { item: PaletteItem; index: number; tier: number } =>
        entry.tier !== undefined,
    )
    .sort(
      (a, b) => a.tier - b.tier || a.item.label.localeCompare(b.item.label) || a.index - b.index,
    )
    .map((entry) => entry.item);
}

interface GotoDef {
  label: string;
  icon: ReactNode;
  view: ActiveView;
}

/** Builds the "Go to" items from the app's places - the caller supplies which ones apply
 * (e.g. Plugins only when there are any, Packs only behind its feature flag). */
export function gotoItems(
  defs: GotoDef[],
  setActiveView: (view: ActiveView) => void,
): PaletteItem[] {
  return defs.map((def) => ({
    id: `goto-${def.view.kind}`,
    section: "goto",
    label: def.label,
    icon: def.icon,
    run: () => setActiveView(def.view),
  }));
}

/** Builds one Skills-section item per installed skill, its secondary text the skill's state
 * group label (e.g. "Needs attention") so the palette reads like a compact version of the table. */
export function skillItems(
  skills: InstalledSkill[],
  groupLabelFor: (skill: InstalledSkill) => string,
  glyphFor: (skill: InstalledSkill) => ReactNode,
  onOpen: (skill: InstalledSkill) => void,
): PaletteItem[] {
  return skills.map((skill) => ({
    id: `skill-${skill.name}`,
    section: "skills",
    label: skill.name,
    secondary: groupLabelFor(skill),
    icon: glyphFor(skill),
    run: () => onOpen(skill),
  }));
}
