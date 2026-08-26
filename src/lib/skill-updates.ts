// ============================================================================
// Skill Studio - skill-updates
// `update-available` moved out of HealthIssue (an update isn't a problem to
// fix, just something to act on) into its own list, shared by Home's
// "Updates" section and the skill page.
// ============================================================================

import type { InstalledSkill, SkillSnapshot } from "./skill-types";

/** Every skill with a newer commit available upstream, per the background update check. */
export function skillsWithUpdates(snapshot: SkillSnapshot): InstalledSkill[] {
  return snapshot.skills.filter((skill) => skill.has_update);
}
