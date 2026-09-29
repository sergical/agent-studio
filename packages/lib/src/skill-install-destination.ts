// ============================================================================
// Skill Studio - skill-install-destination
// Which harnesses an install is for, and the folders that choice writes.
// ============================================================================

import type { AddSkillRequest, AgentId, InstallScope, SkillDestination } from "./skill-types";

export type InstallLinkMode = AddSkillRequest["link_mode"];

interface InstallHarness {
  id: AgentId;
  label: string;
  /** The folder the `skills` CLI links or copies the skill into. `null`
   * for a harness the CLI calls universal: it reads the shared folder. */
  ownFolder: { global: string; project: string } | null;
  /** Reads the shared folder even when it is not chosen. */
  readsSharedFolder: boolean;
  /** Can turn one skill off in its own config (`disabled_harnesses`). */
  hasOffSwitch: boolean;
}

/** First-class harnesses an install can be for, in `AgentId` declaration order. */
export const INSTALL_HARNESSES = [
  {
    id: "claude-code",
    label: "Claude Code",
    ownFolder: { global: "~/.claude/skills", project: ".claude/skills" },
    readsSharedFolder: false,
    hasOffSwitch: false,
  },
  { id: "codex", label: "Codex", ownFolder: null, readsSharedFolder: true, hasOffSwitch: true },
  {
    id: "open-code",
    label: "OpenCode",
    ownFolder: null,
    readsSharedFolder: true,
    hasOffSwitch: true,
  },
  {
    id: "pi",
    label: "pi",
    ownFolder: { global: "~/.pi/agent/skills", project: ".pi/skills" },
    readsSharedFolder: true,
    hasOffSwitch: false,
  },
  { id: "cursor", label: "Cursor", ownFolder: null, readsSharedFolder: true, hasOffSwitch: false },
  {
    id: "grok-build",
    label: "Grok Build",
    ownFolder: { global: "~/.grok/skills", project: ".grok/skills" },
    readsSharedFolder: true,
    hasOffSwitch: false,
  },
] as const satisfies readonly InstallHarness[];

const HARNESS_BY_ID = new Map<AgentId, InstallHarness>(INSTALL_HARNESSES.map((h) => [h.id, h]));

/** The row for one harness, or `undefined` for a harness installs cannot target. */
export function installHarness(id: AgentId): InstallHarness | undefined {
  return HARNESS_BY_ID.get(id);
}

/** Shared-folder path caption for an install scope. */
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
  return INSTALL_HARNESSES.map((row) => row.id).filter((id) => selectedSet.has(id));
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

/** The own folders a choice links or copies into. Claude Code has none to
 * write when its whole `.claude/skills` folder already points at the shared
 * folder. */
function ownFolders(
  chosen: readonly AgentId[],
  scope: InstallScope,
  claudeReadsShared: boolean,
): string[] {
  return chosen.flatMap((id) => {
    const folder = HARNESS_BY_ID.get(id)?.ownFolder;
    if (!folder || (id === "claude-code" && claudeReadsShared)) return [];
    return [folder[scope]];
  });
}

/** Every folder a choice writes: the shared folder first, then each own folder. */
export function installFolders(
  chosen: readonly AgentId[],
  scope: InstallScope,
  claudeReadsShared: boolean,
): string[] {
  return [universalDestinationPath(scope), ...ownFolders(chosen, scope, claudeReadsShared)];
}

/** Link or Copy only means something when the choice writes more than one folder. */
export function linkModeChoiceVisible(
  chosen: readonly AgentId[],
  claudeReadsShared: boolean,
): boolean {
  return ownFolders(chosen, "global", claudeReadsShared).length > 0;
}

function joinPaths(paths: readonly string[]): string {
  if (paths.length <= 1) return paths.join("");
  return `${paths.slice(0, -1).join(", ")} and ${paths[paths.length - 1]}`;
}

/** One sentence that names the folders an install writes. */
export function installFoldersPreview(
  chosen: readonly AgentId[],
  scope: InstallScope,
  linkMode: InstallLinkMode,
  claudeReadsShared: boolean,
): string {
  const shared = universalDestinationPath(scope);
  const own = ownFolders(chosen, scope, claudeReadsShared);
  if (own.length === 0) return `Writes ${shared}.`;
  const verb = linkMode === "copy" ? "copies it to" : "links it in";
  return `Writes ${shared} and ${verb} ${joinPaths(own)}.`;
}

/** Harnesses the picker shows: Claude Code, plus every harness detected on
 * this machine or kept on the first-run screen, in declaration order. */
export function offeredInstallHarnesses(
  detected: readonly AgentId[],
  kept: readonly string[],
): AgentId[] {
  const offered = new Set<string>(["claude-code", ...detected, ...kept]);
  return INSTALL_HARNESSES.map((h) => h.id).filter((id) => offered.has(id));
}

/** A harness that reads the shared folder and has no off switch cannot be
 * left out, and Claude Code cannot be left out when its whole folder points
 * at the shared folder. The off switch lives in the harness's global config,
 * so at project scope it would turn the skill off in every project: there,
 * a harness with no own folder cannot be left out either. */
export function installHarnessLocked(
  id: AgentId,
  claudeReadsShared: boolean,
  scope: InstallScope,
): boolean {
  if (id === "claude-code") return claudeReadsShared;
  const harness = HARNESS_BY_ID.get(id);
  if (!harness || !harness.readsSharedFolder || harness.ownFolder) return false;
  return scope === "project" || !harness.hasOffSwitch;
}

/** The first choice: every offered harness, except pi and Grok Build, which
 * already read the shared folder, so a link in their own folder adds nothing. */
export function defaultInstallHarnesses(offered: readonly AgentId[]): AgentId[] {
  return offered.filter((id) => {
    const harness = HARNESS_BY_ID.get(id);
    return !harness?.ownFolder || !harness.readsSharedFolder;
  });
}

/** The harnesses an install is for: the user's pick (or the default before
 * one), plus every locked harness, in `offered`'s order. */
export function chosenInstallHarnesses(
  offered: readonly AgentId[],
  picked: readonly AgentId[] | null,
  claudeReadsShared: boolean,
  scope: InstallScope,
): AgentId[] {
  const pickedSet = new Set(picked ?? defaultInstallHarnesses(offered));
  return offered.filter(
    (id) => pickedSet.has(id) || installHarnessLocked(id, claudeReadsShared, scope),
  );
}

/** Turn one harness on or off, keeping `offered`'s order. */
export function toggleInstallHarness(
  offered: readonly AgentId[],
  chosen: readonly AgentId[],
  id: AgentId,
  on: boolean,
): AgentId[] {
  const chosenSet = new Set(chosen);
  return offered.filter((other) => (other === id ? on : chosenSet.has(other)));
}

/** Detected harnesses left out that still read the shared folder, so the
 * install turns the skill off in their own config. That config is global,
 * so a project install turns nothing off. */
export function installDisabledHarnesses(
  detected: readonly AgentId[],
  chosen: readonly AgentId[],
  scope: InstallScope,
): AgentId[] {
  if (scope === "project") return [];
  const chosenSet = new Set(chosen);
  return detected.filter((id) => HARNESS_BY_ID.get(id)?.hasOffSwitch && !chosenSet.has(id));
}
