import type { AgentInfo, AgentType } from '../types';

interface AgentSelectorProps {
  agents: AgentInfo[];
  value: AgentType | null;
  onChange: (agent: AgentType) => void;
  recommended?: AgentType | null;
}

export function AgentSelector({ agents, value, onChange, recommended }: AgentSelectorProps) {
  return (
    <div className="agent-selector">
      {agents.map(agent => {
        const isSelected = value === agent.agentType;
        const isRecommended = recommended === agent.agentType;

        return (
          <button
            key={agent.agentType}
            className={`agent-selector-item ${isSelected ? 'selected' : ''} ${!agent.installed ? 'uninstalled' : ''}`}
            onClick={() => agent.installed && onChange(agent.agentType)}
            disabled={!agent.installed}
            title={!agent.installed ? `${agent.displayName} 未安装` : undefined}
          >
            <span className="agent-selector-name">{agent.displayName}</span>
            {!agent.installed && <span className="agent-selector-unavail">未安装</span>}
            {agent.installed && isRecommended && <span className="agent-selector-badge">推荐</span>}
          </button>
        );
      })}
    </div>
  );
}
