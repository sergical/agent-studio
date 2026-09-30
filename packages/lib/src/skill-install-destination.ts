// ============================================================================
// Skill Studio - skill-install-destination
// Which harnesses an install is for, and the folders that choice writes.
// ============================================================================

import type { AddMethod, AddSkillRequest, AgentId, InstallScope } from "./skill-types";

interface InstallHarness {
  id: AgentId;
  label: string;
  /** The harness's own skills folder, the one a per-harness copy writes. */
  folder: { global: string; project: string };
  /** The `skills` CLI puts a link or copy in `folder` next to the shared
   * copy. `false` for a harness it calls universal: it reads the shared
   * folder alone. */
  linksIntoOwnFolder: boolean;
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
    folder: { global: "~/.claude/skills", project: ".claude/skills" },
    linksIntoOwnFolder: true,
    readsSharedFolder: false,
    hasOffSwitch: false,
  },
  {
    id: "codex",
    label: "Codex",
    folder: { global: "~/.codex/skills", project: ".codex/skills" },
    linksIntoOwnFolder: false,
    readsSharedFolder: true,
    hasOffSwitch: true,
  },
  {
    id: "open-code",
    label: "OpenCode",
    folder: { global: "~/.config/opencode/skills", project: ".opencode/skills" },
    linksIntoOwnFolder: false,
    readsSharedFolder: true,
    hasOffSwitch: true,
  },
  {
    id: "pi",
    label: "pi",
    folder: { global: "~/.pi/agent/skills", project: ".pi/skills" },
    linksIntoOwnFolder: true,
    readsSharedFolder: true,
    hasOffSwitch: false,
  },
  {
    id: "cursor",
    label: "Cursor",
    folder: { global: "~/.cursor/skills", project: ".cursor/skills" },
    linksIntoOwnFolder: false,
    readsSharedFolder: true,
    hasOffSwitch: false,
  },
  {
    id: "grok-build",
    label: "Grok Build",
    folder: { global: "~/.grok/skills", project: ".grok/skills" },
    linksIntoOwnFolder: true,
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

/** Without Universal, an install with no harness ticked writes nothing. */
export function installDestinationError(
  universal: boolean,
  chosen: readonly AgentId[],
): string | null {
  return !universal && chosen.length === 0 ? "Select at least one harness." : null;
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

/** With Universal ticked, a harness that reads the shared folder and has no
 * off switch cannot be left out, and Claude Code cannot be left out when its
 * whole folder points at the shared folder. The off switch lives in the
 * harness's global config, so at project scope it would turn the skill off in
 * every project: there, Codex and OpenCode cannot be left out either. With
 * Universal unticked every harness works on its own. */
export function installHarnessLocked(
  id: AgentId,
  claudeReadsShared: boolean,
  scope: InstallScope,
  universal: boolean,
): boolean {
  if (!universal) return false;
  if (id === "claude-code") return claudeReadsShared;
  const harness = HARNESS_BY_ID.get(id);
  if (!harness?.readsSharedFolder) return false;
  return scope === "project" || !harness.hasOffSwitch;
}

/** Why a locked row cannot be unticked, or `null` for a row the user can change. */
export function installHarnessLockReason(
  id: AgentId,
  claudeReadsShared: boolean,
  scope: InstallScope,
  universal: boolean,
): string | null {
  if (!installHarnessLocked(id, claudeReadsShared, scope, universal)) return null;
  const harness = HARNESS_BY_ID.get(id);
  const shared = universalDestinationPath(scope);
  if (id === "claude-code") {
    return `${harness?.folder[scope]} points at ${shared}, so Claude Code reads every skill there.`;
  }
  const limit = harness?.hasOffSwitch
    ? "can't hide one skill in a project"
    : "can't hide one skill";
  return `${harness?.label ?? id} reads ${shared} and ${limit}.`;
}

/** The harnesses an install is for, in `offered`'s order. With Universal
 * ticked: the user's pick (everything before one), plus every locked
 * harness. With Universal unticked: only the pick. */
export function chosenInstallHarnesses(
  offered: readonly AgentId[],
  picked: readonly AgentId[] | null,
  claudeReadsShared: boolean,
  scope: InstallScope,
  universal: boolean,
): AgentId[] {
  const pickedSet = new Set(picked ?? (universal ? offered : []));
  return offered.filter(
    (id) => pickedSet.has(id) || installHarnessLocked(id, claudeReadsShared, scope, universal),
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

/** The ticks that stay when Universal is unticked: the user's own picks. A
 * harness that only reads the shared folder (Cursor, pi, Grok Build) was
 * ticked for that reason alone, so it starts unticked. */
export function harnessesKeptWithoutUniversal(chosen: readonly AgentId[]): AgentId[] {
  return chosen.filter((id) => {
    const harness = HARNESS_BY_ID.get(id);
    return !harness?.readsSharedFolder || harness.hasOffSwitch;
  });
}

/** Offered harnesses left out that still read the shared folder, so the
 * install turns the skill off in their own config. That config is global,
 * so a project install turns nothing off. */
export function installDisabledHarnesses(
  offered: readonly AgentId[],
  chosen: readonly AgentId[],
  scope: InstallScope,
): AgentId[] {
  if (scope === "project") return [];
  const chosenSet = new Set(chosen);
  return offered.filter((id) => HARNESS_BY_ID.get(id)?.hasOffSwitch && !chosenSet.has(id));
}

/** Why Universal cannot be unticked, or `null` when it can. A source with no
 * Copy method (a plain git URL) is dotagents-only, and dotagents writes the
 * shared folder. An empty list means the source is not parsed yet. */
export function universalLockReason(methods: readonly string[]): string | null {
  return methods.length > 0 && !methods.includes("copy")
    ? "This source can only be added with dotagents, which always writes the shared folder."
    : null;
}

/** The method the install runs with. A copy in each harness's own folder is
 * always the Copy method: skills.sh and dotagents write the shared folder. */
export function installMethodFor(method: AddMethod, universal: boolean): AddMethod {
  return universal ? method : "copy";
}

type InstallDestinationFields = Pick<
  AddSkillRequest,
  "method" | "destination" | "agents" | "disabled_harnesses" | "link_mode"
>;

/** The destination half of an install request. Universal on: the shared
 * folder plus a per-skill link for every chosen harness, except pi and Grok
 * Build, which read the shared folder so a link adds nothing. Universal off:
 * one copy in each chosen harness's own folder. */
export function installDestinationFields(input: {
  offered: readonly AgentId[];
  chosen: readonly AgentId[];
  scope: InstallScope;
  method: AddMethod;
  universal: boolean;
}): InstallDestinationFields {
  const { offered, chosen, scope, method, universal } = input;
  if (!universal) {
    return {
      method: installMethodFor(method, false),
      destination: "per-harness",
      agents: [...chosen],
      disabled_harnesses: [],
      link_mode: "copy",
    };
  }
  return {
    method,
    destination: "universal",
    agents: chosen.filter((id) => {
      const harness = HARNESS_BY_ID.get(id);
      return !(harness?.linksIntoOwnFolder && harness.readsSharedFolder);
    }),
    disabled_harnesses: installDisabledHarnesses(offered, chosen, scope),
    link_mode: "link",
  };
}
