// ============================================================================
// Skill Studio - skill-install-destination
// Scope-independent Universal and Per harness install selection.
// ============================================================================

import type { AgentId, InstallScope, SkillDestination } from "./skill-types";

/** First-class harnesses that can receive an independent skill copy. */
export const PER_HARNESS_DESTINATIONS = [
  {
    id: "claude-code",
    label: "Claude Code",
    globalPath: "~/.claude/skills",
    projectPath: ".claude/skills",
  },
  { id: "codex", label: "Codex", globalPath: "~/.codex/skills", projectPath: ".codex/skills" },
  {
    id: "open-code",
    label: "OpenCode",
    globalPath: "~/.config/opencode/skills",
    projectPath: ".opencode/skills",
  },
  { id: "pi", label: "pi", globalPath: "~/.pi/agent/skills", projectPath: ".pi/skills" },
  { id: "cursor", label: "Cursor", globalPath: "~/.cursor/skills", projectPath: ".cursor/skills" },
  {
    id: "grok-build",
    label: "Grok Build",
    globalPath: "~/.grok/skills",
    projectPath: ".grok/skills",
  },
] as const satisfies readonly {
  id: AgentId;
  label: string;
  globalPath: string;
  projectPath: string;
}[];

export type PerHarnessDestinationId = (typeof PER_HARNESS_DESTINATIONS)[number]["id"];

/** Exact path caption for one selectable harness and scope. */
export function perHarnessDestinationPath(
  id: PerHarnessDestinationId,
  scope: InstallScope,
): string {
  const destination = PER_HARNESS_DESTINATIONS.find((row) => row.id === id);
  if (!destination) throw new Error(`Unknown Per harness destination: ${id}`);
  return scope === "global" ? destination.globalPath : destination.projectPath;
}

/** Universal root path caption for an install scope. */
export function universalDestinationPath(scope: InstallScope): string {
  return scope === "global" ? "~/.agents/skills" : ".agents/skills";
}

/** Return a valid install selection in declaration order. */
export function normalizeInstallHarnesses(
  destination: SkillDestination,
  selected: readonly AgentId[],
): AgentId[] {
  if (destination === "universal") {
    return selected.includes("claude-code") ? ["claude-code"] : [];
  }
  const selectedSet = new Set(selected);
  return PER_HARNESS_DESTINATIONS.map((row) => row.id).filter((id) => selectedSet.has(id));
}

/** Per harness is invalid without at least one selected copy target. */
export function installDestinationError(
  destination: SkillDestination,
  selected: readonly AgentId[],
): string | null {
  return destination === "per-harness" &&
    normalizeInstallHarnesses(destination, selected).length === 0
    ? "Select at least one harness."
    : null;
}

/** Trials require one Universal deployment and cannot target independent harness copies. */
export function installTrialError(destination: SkillDestination, trial: boolean): string | null {
  return destination === "per-harness" && trial
    ? "24-hour trials are available only for Universal installs."
    : null;
}

/** Clear a selected trial when the user changes Destination to Per harness. */
export function trialSelectionForDestination(
  destination: SkillDestination,
  selected: boolean,
): boolean {
  return destination === "universal" && selected;
}
