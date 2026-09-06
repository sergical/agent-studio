import type { AgentId } from "@skill-studio/lib";

/** Harness names shown by the Universal install visibility selector. */
export const UNIVERSAL_VISIBILITY_HARNESSES = [
  ["claude-code", "Claude Code"],
  ["codex", "Codex"],
  ["open-code", "OpenCode"],
  ["pi", "pi"],
  ["cursor", "Cursor"],
  ["grok-build", "Grok Build"],
] as const satisfies readonly (readonly [AgentId, string])[];

/** Harnesses that read the Universal folder and have no per-skill off switch. */
const NO_PER_SKILL_DISABLE = new Set<AgentId>(["pi", "cursor", "grok-build"]);

/** Whether a harness can disable one skill in its own configuration. */
export function harnessHasPerSkillDisable(agent: AgentId): boolean {
  return !NO_PER_SKILL_DISABLE.has(agent);
}

/** Build the ordered harness list for a Universal install. */
export function universalInstallHarnesses(
  enabledReaders: readonly AgentId[],
  claudeLink: boolean,
): AgentId[] {
  return claudeLink ? [...enabledReaders, "claude-code"] : [...enabledReaders];
}

/** Find harnesses that need disabling after a Universal install. */
export function universalDisabledHarnesses(
  readers: readonly AgentId[],
  enabledReaders: readonly AgentId[],
  claudeReadsShared: boolean,
  claudeLink: boolean,
): AgentId[] {
  const enabledReaderSet = new Set(enabledReaders);
  const disabled = readers.filter((id) => !enabledReaderSet.has(id));
  if (claudeReadsShared && !claudeLink) disabled.push("claude-code");
  return disabled;
}
