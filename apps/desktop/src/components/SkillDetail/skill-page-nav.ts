// ============================================================================
// skill-page-nav - Shared navigation helper for the skill detail page's
// breadcrumb: both SkillPage.tsx and InstalledSkillHeader.tsx import it.
// ============================================================================

import type { ActiveView } from "../../store/appStore";

/** The breadcrumb's parent label: the name of the view the page was opened from. */
export function backLabel(from: ActiveView): string {
  switch (from.kind) {
    case "home":
      return "Home";
    case "skills":
      return "Skills";
    case "activity":
      return "Activity";
    default:
      // `ActiveView`'s "skill" kind never nests as its own `from` (see `openSkill`).
      return "Back";
  }
}
