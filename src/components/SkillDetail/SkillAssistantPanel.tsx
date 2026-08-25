// ============================================================================
// SkillAssistantPanel - Placeholder right-column card on the skill page: a
// harness picker and three not-yet-wired actions (Audit, Test, Apply)
// ============================================================================

import { skillVisibleToAgent } from "../../lib/skill-coverage";
import { COMMON_AGENTS } from "../../lib/skill-types";
import type { AgentId, InstalledSkill } from "../../lib/skill-types";
import { HarnessSegmentedControl } from "../ui/HarnessSegmentedControl";

interface SkillAssistantPanelProps {
  skill: InstalledSkill;
}

/** Every first-class agent that can actually see `skill`, in `COMMON_AGENTS` order. */
function visibleAgentsFor(skill: InstalledSkill): AgentId[] {
  return COMMON_AGENTS.filter((agent) => skillVisibleToAgent(skill, agent) !== "none");
}

/**
 * A surface reserved for the coming per-skill assistant: a disabled harness
 * picker (defaulting to Claude Code when it sees the skill, else the first
 * harness that does) and three disabled action buttons. No behavior yet.
 */
export function SkillAssistantPanel({ skill }: SkillAssistantPanelProps) {
  const visibleAgents = visibleAgentsFor(skill);
  const selected: AgentId =
    (visibleAgents.includes("claude-code") ? "claude-code" : visibleAgents[0]) ?? "claude-code";

  return (
    <div className="skill-assistant-panel">
      <div className="skill-assistant-panel-label">Assistant</div>

      <HarnessSegmentedControl selected={selected} visibleAgents={new Set(visibleAgents)} />

      <div className="skill-assistant-panel-actions">
        <button type="button" className="skill-action-button" disabled>
          Audit
        </button>
        <button type="button" className="skill-action-button" disabled>
          Test
        </button>
        <button type="button" className="skill-action-button" disabled>
          Apply
        </button>
      </div>

      <p className="skill-assistant-panel-note">
        Runs on your local harness. Coming in the next update.
      </p>
    </div>
  );
}
