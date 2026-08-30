// ============================================================================
// AgentTargetSelector - Multi-select for agent targets
// ============================================================================

import { useState, useEffect } from "react";
import { Check, ChevronDown, ChevronUp } from "lucide-react";
import { getAgentTargets } from "../../lib/skill-api";
import type { AgentId, AgentTarget } from "@skill-studio/lib";
import { COMMON_AGENTS } from "@skill-studio/lib";

interface AgentTargetSelectorProps {
  selectedAgents: AgentId[];
  onChange: (agents: AgentId[]) => void;
  disabled?: boolean;
}

export function AgentTargetSelector({
  selectedAgents,
  onChange,
  disabled = false,
}: AgentTargetSelectorProps) {
  const [agents, setAgents] = useState<AgentTarget[]>([]);
  const [isExpanded, setIsExpanded] = useState(false);
  const [isLoading, setIsLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;

    getAgentTargets()
      .then((targets) => {
        if (!cancelled) {
          setAgents(targets);
        }
      })
      .finally(() => {
        if (!cancelled) {
          setIsLoading(false);
        }
      });

    return () => {
      cancelled = true;
    };
  }, []);

  const toggleAgent = (agentId: AgentId) => {
    if (disabled) return;

    if (selectedAgents.includes(agentId)) {
      onChange(selectedAgents.filter((id) => id !== agentId));
    } else {
      onChange([...selectedAgents, agentId]);
    }
  };

  const selectAll = () => {
    if (disabled) return;
    onChange(agents.map((a) => a.id));
  };

  const selectNone = () => {
    if (disabled) return;
    onChange([]);
  };

  const selectCommon = () => {
    if (disabled) return;
    onChange(COMMON_AGENTS.filter((id) => agents.some((a) => a.id === id)));
  };

  // Separate common agents from others
  const commonAgents = agents.filter((a) => COMMON_AGENTS.includes(a.id));
  const otherAgents = agents.filter((a) => !COMMON_AGENTS.includes(a.id));

  if (isLoading) {
    return (
      <div className="flex flex-col gap-3">
        <span>Loading agents…</span>
      </div>
    );
  }

  return (
    <div className={`flex flex-col gap-3 ${disabled ? "pointer-events-none opacity-50" : ""}`}>
      <div className="flex items-center justify-between gap-2">
        <span className="text-small font-medium text-text-secondary">
          Install to agents ({selectedAgents.length} selected)
        </span>
        <div className="flex gap-1">
          <button
            type="button"
            className="rounded-xs border border-border bg-transparent px-2 py-1 text-caption font-medium text-text-tertiary transition-colors hover:bg-bg-hover hover:text-text-secondary"
            onClick={selectCommon}
            disabled={disabled}
          >
            Common
          </button>
          <button
            type="button"
            className="rounded-xs border border-border bg-transparent px-2 py-1 text-caption font-medium text-text-tertiary transition-colors hover:bg-bg-hover hover:text-text-secondary"
            onClick={selectAll}
            disabled={disabled}
          >
            All
          </button>
          <button
            type="button"
            className="rounded-xs border border-border bg-transparent px-2 py-1 text-caption font-medium text-text-tertiary transition-colors hover:bg-bg-hover hover:text-text-secondary"
            onClick={selectNone}
            disabled={disabled}
          >
            None
          </button>
        </div>
      </div>

      <div className="flex flex-wrap gap-1.5">
        {commonAgents.map((agent) => (
          <button
            key={agent.id}
            type="button"
            className={`flex items-center gap-1 rounded-xs border px-2.5 py-1.5 text-caption font-medium transition-colors ${
              selectedAgents.includes(agent.id)
                ? "border-accent bg-accent-softer text-accent"
                : "border-border bg-bg-primary text-text-secondary hover:border-border-focus"
            }`}
            onClick={() => toggleAgent(agent.id)}
            disabled={disabled}
          >
            {selectedAgents.includes(agent.id) && <Check size={12} />}
            <span>{agent.name}</span>
          </button>
        ))}
      </div>

      <button
        type="button"
        className="flex items-center gap-1.5 rounded-sm border border-dashed border-border bg-transparent p-2 text-caption text-text-tertiary transition-colors hover:bg-bg-hover hover:text-text-secondary"
        onClick={() => setIsExpanded(!isExpanded)}
        disabled={disabled}
      >
        {isExpanded ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
        <span>{isExpanded ? "Show less" : `Show all ${agents.length} agents`}</span>
      </button>

      {isExpanded && (
        <div className="flex flex-wrap gap-1.5">
          {otherAgents.map((agent) => (
            <button
              key={agent.id}
              type="button"
              className={`flex items-center gap-1 rounded-xs border px-2.5 py-1.5 text-caption font-medium transition-colors ${
                selectedAgents.includes(agent.id)
                  ? "border-accent bg-accent-softer text-accent"
                  : "border-border bg-bg-primary text-text-secondary hover:border-border-focus"
              }`}
              onClick={() => toggleAgent(agent.id)}
              disabled={disabled}
            >
              {selectedAgents.includes(agent.id) && <Check size={12} />}
              <span>{agent.name}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
