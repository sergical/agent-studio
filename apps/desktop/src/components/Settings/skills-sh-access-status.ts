// ============================================================================
// Skill Studio - skills.sh access status
// Models whether Settings has authoritative direct/server access information.
// ============================================================================

import type { SkillsShAccessInfo } from "@skill-studio/lib";

export type SkillsShAccessState =
  | { kind: "loading" }
  | { kind: "available"; access: SkillsShAccessInfo }
  | { kind: "unavailable" };

/** Returns Settings status copy only when loading has finished. */
export function skillsShAccessStatusText(state: SkillsShAccessState): string | null {
  if (state.kind === "loading") return null;
  if (state.kind === "unavailable") return "skills.sh access status is unavailable.";
  if (state.access.mode === "direct") {
    return "Using a local skills.sh key (developer override)";
  }
  return state.access.server_url
    ? `Browsing through the Skill Studio server at ${state.access.server_url}`
    : "skills.sh access status is unavailable.";
}
