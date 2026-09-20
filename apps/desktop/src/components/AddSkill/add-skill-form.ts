// ============================================================================
// Skill Studio - Add Skill form domain logic
// ============================================================================

import { installDestinationError } from "@skill-studio/lib";
import type {
  AddMethod,
  AddMethodDefaults,
  AgentId,
  GithubSkillEntry,
  InstallScope,
  ParsedSkillSource,
  SkillDestination,
} from "@skill-studio/lib";

/** An Add Skill method, including the separate pack import flow. */
export type AddSkillSheetMethod = AddMethod | "pack";

/**
 * List a parsed source's install methods in preferred order. Git requires
 * dotagents, while GitHub and local sources retain their existing fallback
 * methods. Until defaults load, dotagents stays available as before.
 */
export function availableAddSkillMethods(
  parsed: ParsedSkillSource | { error: string },
  defaults: AddMethodDefaults | null,
): AddSkillSheetMethod[] {
  if ("error" in parsed) return [];
  const dotagentsInstalled = defaults?.dotagents_installed ?? true;
  if (parsed.kind === "github") {
    // Pack is withheld here: import_skill_pack, confirm_skill_pack_trust, and
    // abandon_pack_import_trust are unregistered in lib.rs. Restore "pack" to
    // these lists together with registeredInLibRs: true in skill-api.test.ts.
    return dotagentsInstalled ? ["dotagents", "skills-sh", "copy"] : ["skills-sh", "copy"];
  }
  if (parsed.kind === "git") return dotagentsInstalled ? ["dotagents"] : [];
  return ["copy"];
}

/** Validate every persistent gate used by Add Skill submission and its footer button. */
export function isAddSkillFormValid(input: {
  parsed: ParsedSkillSource | { error: string };
  noMethodsAvailable: boolean;
  destination: SkillDestination;
  agents: readonly AgentId[];
  scope: InstallScope;
  projectPath: string | null;
  githubEntries: readonly GithubSkillEntry[] | null;
}): boolean {
  const { parsed, noMethodsAvailable, destination, agents, scope, projectPath, githubEntries } =
    input;
  return (
    !("error" in parsed) &&
    !noMethodsAvailable &&
    (parsed.kind !== "git" || !!parsed.skillName?.trim()) &&
    (scope !== "project" || !!projectPath) &&
    installDestinationError(destination, agents) === null &&
    (githubEntries === null || githubEntries.length > 0)
  );
}
