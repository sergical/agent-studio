// ============================================================================
// Skill Studio - Add Skill form domain logic
// ============================================================================

import { installDestinationError, installTrialError } from "@skill-studio/lib";
import type {
  AddMethod,
  AddMethodDefaults,
  AgentId,
  GithubSkillEntry,
  InstallScope,
  ParsedSkillSource,
  SkillDestination,
} from "@skill-studio/lib";
import { isFeatureEnabled } from "../../lib/feature-flags";

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
    return dotagentsInstalled
      ? ["dotagents", "skills-sh", "copy", "pack"]
      : ["skills-sh", "copy", "pack"];
  }
  if (parsed.kind === "git") return dotagentsInstalled ? ["dotagents"] : [];
  return isFeatureEnabled("skill-packs") ? ["copy", "pack"] : ["copy"];
}

/** Validate every persistent gate used by Add Skill submission and its footer button. */
export function isAddSkillFormValid(input: {
  parsed: ParsedSkillSource | { error: string };
  noMethodsAvailable: boolean;
  destination: SkillDestination;
  agents: readonly AgentId[];
  scope: InstallScope;
  projectPath: string | null;
  trial: boolean;
  githubEntries: readonly GithubSkillEntry[] | null;
}): boolean {
  const {
    parsed,
    noMethodsAvailable,
    destination,
    agents,
    scope,
    projectPath,
    trial,
    githubEntries,
  } = input;
  return (
    !("error" in parsed) &&
    !noMethodsAvailable &&
    (scope !== "project" || !!projectPath) &&
    installDestinationError(destination, agents) === null &&
    installTrialError(destination, trial) === null &&
    (githubEntries === null || githubEntries.length > 0)
  );
}
