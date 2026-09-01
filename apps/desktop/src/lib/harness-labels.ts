// ============================================================================
// harness-labels - Shared harness-label data (Claude Code / Codex / OpenCode
// / pi), consumed by SkillAssistantPanel and SkillHistorySection.
// ============================================================================

import type { AgentId } from "@skill-studio/lib";

/** Display label for each first-class harness, in the order the control shows them. */
export const HARNESS_LABELS = [
  ["claude-code", "Claude Code"],
  ["codex", "Codex"],
  ["open-code", "OpenCode"],
  ["pi", "pi"],
] satisfies [AgentId, string][];
