// ============================================================================
// Skill Studio - skill-split-model
// Pure helpers for the "Split into harness folders…" dialog: which harnesses
// it offers, which start checked, and which folders the split will write.
// ============================================================================

import { deploymentLabelFromAgentId } from "@skill-studio/lib";
import type { AgentId, SplitCopy } from "@skill-studio/lib";
import type { ScopeGroup } from "./skill-location-status";

/** Every harness the core `split` op can write a copy for, in dialog order. */
export const SPLIT_HARNESSES: AgentId[] = [
  "claude-code",
  "codex",
  "open-code",
  "pi",
  "cursor",
  "grok-build",
];

/** Same text as the core's `ops_split::SPLIT_UPDATE_NOTE`, shown before the split runs. */
export const SPLIT_UPDATE_NOTE =
  "npx skills update only updates the Universal copy, so these copies no longer get its updates.";

/** Harnesses that read the scope's Universal folder now - the dialog checks these first. */
export function splitReaders(group: ScopeGroup): AgentId[] {
  const reading = new Set<AgentId>();
  for (const row of group.rows) {
    if (row.kind === "reader" || row.kind === "link") reading.add(row.harness);
  }
  return SPLIT_HARNESSES.filter((harness) => reading.has(harness));
}

export interface SplitFolderRow {
  harness: AgentId;
  label: string;
  path: string;
}

/** The folders a split writes for the checked harnesses, in `SPLIT_HARNESSES` order. */
export function splitFolderRows(
  targets: SplitCopy[],
  checked: ReadonlySet<AgentId>,
): SplitFolderRow[] {
  const pathByHarness = new Map(targets.map((target) => [target.harness, target.path]));
  return SPLIT_HARNESSES.flatMap((harness) => {
    const path = pathByHarness.get(harness);
    if (!checked.has(harness) || path === undefined) return [];
    return [{ harness, label: deploymentLabelFromAgentId(harness), path }];
  });
}
