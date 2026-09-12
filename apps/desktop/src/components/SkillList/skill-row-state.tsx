// ============================================================================
// skill-row-state - The single highest-ranked thing a row needs to say about
// a skill, and `whereFacts`, the disk-location/harness-reach model every
// row renders.
// ============================================================================

import {
  AGENT_MATRIX_LABELS,
  driftingCopies,
  isBlockingSpecViolation,
  locationSummary,
  parentDirectory,
  trialHoursLeft,
} from "@skill-studio/lib";
import type { AgentId, Deployment, InstalledSkill } from "@skill-studio/lib";
import { buildScopeGroups, skillRollup } from "../SkillDetail/skill-location-status";
import { harnessIdFromLabel } from "../ui/HarnessIcon";

export type RowLevel = "error" | "warning" | "info" | "muted";
/** Which ladder rung produced the state; picks the glyph in SkillRowCells. */
type RowKind = "violation" | "rollup" | "trial" | "update" | "parked";

export interface RowState {
  kind: RowKind;
  level: RowLevel;
  label: string;
  detail: string | null;
  action: string | null;
}

/** "Parked · Aug 25, 2026" / "Parked" — copied from InstalledSkillHeader's chip. */
function parkedChipLabel(parkedAt: string | null | undefined): string {
  if (!parkedAt) return "Parked";
  const date = new Date(parkedAt);
  if (Number.isNaN(date.getTime())) return "Parked";
  return `Parked · ${date.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" })}`;
}

/** "Trial · 17 h" / "Trial · <1 h" / "Trial · expired" — copied from InstalledSkillHeader's chip. */
function trialChipLabel(expiresAt: string): string {
  const hours = trialHoursLeft(expiresAt);
  if (hours < 0) return "Trial · expired";
  if (hours < 1) return "Trial · <1 h";
  return `Trial · ${hours} h`;
}

/** The one thing this row most needs to say, in ladder order: blocking spec
 * violation, then the folder rollup, then a trial, an update, or parked.
 * `null` when the skill is unremarkable. */
export function rowState(skill: InstalledSkill): RowState | null {
  const blocking = skill.spec_violations.filter(isBlockingSpecViolation);
  if (blocking.length > 0) {
    return {
      kind: "violation",
      level: "error",
      label: "Blocking spec violation",
      detail: blocking[0],
      action: "Fix YAML",
    };
  }

  const rollup = skillRollup(skill, buildScopeGroups(skill));
  const [first, second] = rollup.tip.split("\n");
  if (rollup.level === "error") {
    return {
      kind: "rollup",
      level: "error",
      label: first,
      detail: second ?? null,
      action: "Repair",
    };
  }
  if (rollup.level === "warning") {
    const hasDrift = driftingCopies(locationSummary(skill)).length > 0;
    return {
      kind: "rollup",
      level: "warning",
      label: first,
      detail: second ?? null,
      action: hasDrift ? "Compare" : null,
    };
  }

  const [trial] = skill.trials;
  if (trial) {
    return {
      kind: "trial",
      level: "info",
      label: trialChipLabel(trial.expires_at),
      detail: null,
      action: "Keep",
    };
  }
  if (skill.update_owner_ids.length > 0) {
    return {
      kind: "update",
      level: "info",
      label: "Update available",
      detail: null,
      action: "Update",
    };
  }
  if (skill.parked) {
    return {
      kind: "parked",
      level: "muted",
      label: parkedChipLabel(skill.parked_at),
      detail: null,
      action: "Unpark",
    };
  }
  return null;
}

type DiskLocationKind = "global" | "project";

/** One deployment's fact sheet, for a `DiskLocation`'s or `HarnessReach`'s
 * per-deployment line. `scope`/`resolvedPath` back a linked entry's
 * "→ target" line and a Globe-vs-project icon. */
interface LocationEntry {
  harness: AgentId | "shared";
  label: string;
  path: string;
  how: How;
  readOnly: boolean;
  disabled: boolean;
  scope: DiskLocationKind;
  resolvedPath: string | null;
}

/** How one deployment reaches the skill: the Universal folder's own copy, a
 * symlink into it, an own copy elsewhere, or a symlink that no longer
 * resolves. */
type How = "universal" | "linked" | "own" | "broken";

/** One place a skill lives on disk - the Universal folder's global scope, or
 * a project root - and every deployment installed there. */
export interface DiskLocation {
  kind: DiskLocationKind;
  name: string;
  path: string;
  entries: LocationEntry[];
}

/** One first-class harness, whether it reaches the skill at all, and how
 * (worst deployment wins), for the Harnesses group. */
export interface HarnessReach {
  harness: AgentId;
  label: string;
  reached: boolean;
  how: How | null;
  disabled: boolean;
  entries: LocationEntry[];
}

/** The Universal folder (`~/.agents/skills`) itself: whether the skill has a
 * copy there. */
export interface Universal {
  present: boolean;
  path: string | null;
}

interface WhereFacts {
  locations: DiskLocation[];
  universal: Universal;
  harnesses: HarnessReach[];
}

/** One harness `whereFacts` can render, in the order the Harnesses group
 * draws it. */
export interface HarnessListEntry {
  label: string;
  harness: AgentId;
}

/** `AGENT_MATRIX_LABELS`'s six first-class agents, in order - `whereFacts`'
 * default list. */
export const DEFAULT_HARNESS_LIST: HarnessListEntry[] = AGENT_MATRIX_LABELS.flatMap((label) => {
  const harness = harnessIdFromLabel(label);
  return harness && harness !== "shared" ? [{ label, harness }] : [];
});

/** `shared` sorts first, then `harnessList` order - the Universal folder is
 * the source every harness reaches through, so it leads a location's or
 * harness list's entries. */
function harnessRank(harness: AgentId | "shared", harnessList: HarnessListEntry[]): number {
  if (harness === "shared") return -1;
  return harnessList.findIndex((entry) => entry.harness === harness);
}

/** Resolves a deployment's `agent` display label to a harness id: the six
 * first-class labels `harnessIdFromLabel` knows, or a label match against
 * the harness list itself. */
function resolveHarness(
  agentLabel: string,
  harnessList: HarnessListEntry[],
): AgentId | "shared" | null {
  return (
    harnessIdFromLabel(agentLabel) ??
    harnessList.find((entry) => entry.label === agentLabel)?.harness ??
    null
  );
}

/** How one deployment reaches the skill: a dangling symlink is "broken"
 * first, else the Universal folder's own copy (or the shared root itself) is
 * "universal", else a symlink into it is "linked", else it's its own
 * independent copy. */
function deploymentHow(d: Deployment, harness: AgentId | "shared"): How {
  if (d.is_symlink && d.resolved_path === null) return "broken";
  if (harness === "shared" || d.backing.kind === "canonical") return "universal";
  if (
    d.backing.kind === "linked-to" ||
    (d.is_symlink && /\/\.agents\/skills\//.test(d.resolved_path ?? ""))
  )
    return "linked";
  return "own";
}

function locationEntry(d: Deployment, harness: AgentId | "shared"): LocationEntry {
  return {
    harness,
    label: harness === "shared" ? "Universal folder" : d.agent,
    path: d.path,
    how: deploymentHow(d, harness),
    readOnly: d.mutability === "read-only",
    disabled: d.disabled,
    scope: d.scope === "project" && d.project_path ? "project" : "global",
    resolvedPath: d.resolved_path ?? null,
  };
}

/** Worst-wins ordering for a `HarnessReach`'s single `how`: a broken link is
 * worse news than a healthy one anywhere else. */
const HOW_SEVERITY = { broken: 3, linked: 2, own: 1, universal: 0 } satisfies Record<How, number>;

/** Splits a skill's deployments into the disk locations they sit in and the
 * harnesses that reach them, plus the Universal folder's own facts.
 * `harnessList` defaults to `DEFAULT_HARNESS_LIST`'s six. */
export function whereFacts(
  skill: InstalledSkill,
  harnessList: HarnessListEntry[] = DEFAULT_HARNESS_LIST,
): WhereFacts {
  const globalDeployments = skill.deployments.filter((d) => d.scope !== "project");

  const locationsByKey = new Map<string, DiskLocation>();
  if (globalDeployments.length > 0) {
    locationsByKey.set("global", { kind: "global", name: "Global", path: "~", entries: [] });
  }
  for (const d of skill.deployments) {
    if (d.scope !== "project" || !d.project_path) continue;
    const key = `project:${d.project_path}`;
    if (!locationsByKey.has(key)) {
      locationsByKey.set(key, {
        kind: "project",
        name: d.project_path.split("/").filter(Boolean).pop() ?? d.project_path,
        path: d.project_path,
        entries: [],
      });
    }
  }

  let universalPresent = false;
  let universalPath: string | null = null;

  for (const d of skill.deployments) {
    const harness = resolveHarness(d.agent, harnessList);
    if (!harness) continue;
    const entry = locationEntry(d, harness);
    if (harness === "shared") {
      universalPresent = true;
      universalPath = parentDirectory(d.path);
    }
    const key = d.scope === "project" && d.project_path ? `project:${d.project_path}` : "global";
    locationsByKey.get(key)?.entries.push(entry);
  }

  const locations = [...locationsByKey.values()]
    .map((location) => ({
      ...location,
      entries: [...location.entries].sort(
        (a, b) => harnessRank(a.harness, harnessList) - harnessRank(b.harness, harnessList),
      ),
    }))
    .sort((a, b) => {
      if (a.kind === "global") return -1;
      if (b.kind === "global") return 1;
      return a.name.localeCompare(b.name);
    });

  const harnesses: HarnessReach[] = harnessList.map(({ harness, label }) => {
    // Global first, then projects - the same order the harness's own tooltip lines read.
    const entries = locations.flatMap((l) => l.entries.filter((e) => e.harness === harness));
    const reached = entries.length > 0;
    const how = reached
      ? entries.reduce<How>(
          (worst, e) => (HOW_SEVERITY[e.how] > HOW_SEVERITY[worst] ? e.how : worst),
          entries[0].how,
        )
      : null;
    return {
      harness,
      label,
      reached,
      how,
      disabled: reached && entries.every((e) => e.disabled),
      entries,
    };
  });

  return {
    locations,
    universal: {
      present: universalPresent,
      path: universalPath,
    },
    harnesses,
  };
}

/** The fix or fixes each ladder rung offers from the glyph, independent of
 * `state.action`. */
export function fixesFor(state: RowState): string[] {
  switch (state.kind) {
    case "violation":
      return ["Fix YAML"];
    case "rollup":
      return state.level === "error" ? ["Fix link"] : [state.action ?? "Compare", "Convert"];
    case "trial":
      return ["Keep"];
    case "update":
      return ["Pull latest"];
    case "parked":
      return ["Unpark"];
  }
}

/** Whether the row has a state worth surfacing a glyph or menu for - every
 * kind except parked, which is a quiet fact rather than something to decide
 * about. */
export function isDecision(state: RowState | null): boolean {
  return state !== null && state.kind !== "parked";
}
