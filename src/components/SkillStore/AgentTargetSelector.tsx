// ============================================================================
// AgentTargetSelector - Multi-select for agent targets
// ============================================================================

import { useState, useEffect } from 'react';
import { Check, ChevronDown, ChevronUp } from 'lucide-react';
import { getAgentTargets } from '../../lib/skillsApi';
import type { AgentId, AgentTarget } from '../../lib/skillsTypes';
import { COMMON_AGENTS } from '../../lib/skillsTypes';

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
    loadAgents();
  }, []);

  const loadAgents = async () => {
    try {
      const targets = await getAgentTargets();
      setAgents(targets);
    } catch (err) {
      console.error('Failed to load agent targets:', err);
    } finally {
      setIsLoading(false);
    }
  };

  const toggleAgent = (agentId: AgentId) => {
    if (disabled) return;

    if (selectedAgents.includes(agentId)) {
      onChange(selectedAgents.filter(id => id !== agentId));
    } else {
      onChange([...selectedAgents, agentId]);
    }
  };

  const selectAll = () => {
    if (disabled) return;
    onChange(agents.map(a => a.id));
  };

  const selectNone = () => {
    if (disabled) return;
    onChange([]);
  };

  const selectCommon = () => {
    if (disabled) return;
    onChange(COMMON_AGENTS.filter(id => agents.some(a => a.id === id)));
  };

  // Separate common agents from others
  const commonAgents = agents.filter(a => COMMON_AGENTS.includes(a.id));
  const otherAgents = agents.filter(a => !COMMON_AGENTS.includes(a.id));

  if (isLoading) {
    return (
      <div className="agent-selector agent-selector-loading">
        <span>Loading agents...</span>
      </div>
    );
  }

  return (
    <div className={`agent-selector ${disabled ? 'disabled' : ''}`}>
      <div className="agent-selector-header">
        <span className="agent-selector-label">
          Install to agents ({selectedAgents.length} selected)
        </span>
        <div className="agent-selector-actions">
          <button
            type="button"
            className="agent-selector-action"
            onClick={selectCommon}
            disabled={disabled}
          >
            Common
          </button>
          <button
            type="button"
            className="agent-selector-action"
            onClick={selectAll}
            disabled={disabled}
          >
            All
          </button>
          <button
            type="button"
            className="agent-selector-action"
            onClick={selectNone}
            disabled={disabled}
          >
            None
          </button>
        </div>
      </div>

      <div className="agent-selector-common">
        {commonAgents.map(agent => (
          <button
            key={agent.id}
            type="button"
            className={`agent-chip ${selectedAgents.includes(agent.id) ? 'selected' : ''}`}
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
        className="agent-selector-expand"
        onClick={() => setIsExpanded(!isExpanded)}
        disabled={disabled}
      >
        {isExpanded ? <ChevronUp size={14} /> : <ChevronDown size={14} />}
        <span>{isExpanded ? 'Show less' : `Show all ${agents.length} agents`}</span>
      </button>

      {isExpanded && (
        <div className="agent-selector-all">
          {otherAgents.map(agent => (
            <button
              key={agent.id}
              type="button"
              className={`agent-chip ${selectedAgents.includes(agent.id) ? 'selected' : ''}`}
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
