// ============================================================================
// HarnessSegmentedControl - Four-button harness toggle (Claude Code / Codex /
// OpenCode / pi), used by SkillAssistantPanel. "Unavailable" harnesses (ones
// that don't see the current skill's own deployments) render dimmed with a
// title explaining why, but stay selectable - the assistant runs in its own
// scratch directory, independent of installed deployments.
// ============================================================================

import type { AgentId } from "../../lib/skill-types";
import { HarnessIcon } from "./HarnessIcon";

/** Display label for each first-class harness, in the order the control shows them. */
const HARNESS_LABELS = [
  ["claude-code", "Claude Code"],
  ["codex", "Codex"],
  ["open-code", "OpenCode"],
  ["pi", "pi"],
] satisfies [AgentId, string][];

interface HarnessSegmentedControlProps {
  /** The currently-selected harness, highlighted like the active item elsewhere. */
  selected: AgentId;
  /** Harnesses that actually see the skill; the rest render dimmed. */
  visibleAgents: ReadonlySet<AgentId>;
  /** Omit to render every button disabled, as a read-only placeholder. */
  onSelect?: (agent: AgentId) => void;
}

/**
 * Harness picker: one button per first-class agent. `selected` gets the
 * usual "active" styling; harnesses outside `visibleAgents` get an
 * additional dimmed style and a title explaining they don't see this skill.
 */
export function HarnessSegmentedControl({
  selected,
  visibleAgents,
  onSelect,
}: HarnessSegmentedControlProps) {
  return (
    <div className="harness-segmented-control">
      {HARNESS_LABELS.map(([agent, label]) => {
        const isVisible = visibleAgents.has(agent);
        return (
          <button
            key={agent}
            type="button"
            disabled={!onSelect}
            onClick={() => onSelect?.(agent)}
            aria-pressed={agent === selected}
            className={`harness-segmented-control-item ${agent === selected ? "active" : ""} ${
              isVisible ? "" : "unavailable"
            }`}
            title={isVisible ? undefined : `${label} doesn't see this skill`}
          >
            <HarnessIcon harness={agent} size={12} />
            {label}
          </button>
        );
      })}
    </div>
  );
}
