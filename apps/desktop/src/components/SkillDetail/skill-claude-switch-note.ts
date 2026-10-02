// ============================================================================
// skill-claude-switch-note - the plain-words note about Claude Code's on/off
// switch. Claude Code turns a skill off by its name in ~/.claude/settings.json,
// so one change applies in every project. Pure, so a test can pin the wording.
// ============================================================================

import { agentIdFromDeploymentLabel, basename } from "@skill-studio/lib";
import type { Deployment, InstalledSkill, Toast } from "@skill-studio/lib";

/** The caption and tooltip on a Claude Code row's switch. */
export const CLAUDE_CODE_SWITCH_NOTE =
  "Claude Code turns skills off by name, so this applies in every project.";

/** Names of the projects whose own Claude Code copy of the skill the change also reaches, sorted, once each. */
export function claudeCodeProjectNames(deployments: Deployment[]): string[] {
  const names = new Set<string>();
  for (const d of deployments) {
    if (agentIdFromDeploymentLabel(d.agent) === "claude-code" && d.project_path) {
      names.add(basename(d.project_path));
    }
  }
  return [...names].sort();
}

/** The success toast after Claude Code's switch changed: says what happened and names the projects it reaches. */
export function claudeCodeSwitchToast(
  skill: Pick<InstalledSkill, "name" | "deployments">,
  enabled: boolean,
): Omit<Toast, "id"> {
  const projects = claudeCodeProjectNames(skill.deployments);
  const note =
    projects.length > 0
      ? `Claude Code turns skills off by name, so this also applies in ${projects.join(", ")}.`
      : CLAUDE_CODE_SWITCH_NOTE;
  return {
    type: "success",
    title: `${skill.name} is ${enabled ? "on" : "off"} for Claude Code`,
    message: note,
  };
}
