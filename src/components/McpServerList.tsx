import type { McpServerConfig, AgentType } from '../types';

const ALL_AGENTS: { value: AgentType; label: string }[] = [
  { value: 'claude_code', label: 'Claude Code' },
  { value: 'codex', label: 'Codex' },
  { value: 'cursor_agent', label: 'Cursor Agent' },
  { value: 'gemini_cli', label: 'Gemini CLI' },
  { value: 'copilot', label: 'Copilot' },
  { value: 'qwen_code', label: 'Qwen Code' },
  { value: 'pi', label: 'Pi' },
];

interface McpServerListProps {
  servers: McpServerConfig[];
  editIdx: number | null;
  onToggle: (idx: number) => void;
  onRemove: (idx: number) => void;
  onEdit: (idx: number) => void;
  onUpdateTarget: (idx: number, agent: AgentType, checked: boolean) => void;
}

export function McpServerList({ servers, editIdx, onToggle, onRemove, onEdit, onUpdateTarget }: McpServerListProps) {
  if (servers.length === 0) {
    return <p className="config-center-placeholder">暂无 MCP Server 配置。在下方添加新的 Server。</p>;
  }

  return (
    <div className="config-server-list">
      {servers.map((server, idx) => (
        <div key={server.name} className={`config-server-item ${server.enabled ? '' : 'disabled'}`}>
          <div className="config-server-item-main">
            <button
              className="config-server-toggle"
              onClick={() => onToggle(idx)}
              title={server.enabled ? '禁用' : '启用'}
            >
              {server.enabled ? '●' : '○'}
            </button>
            <div className="config-server-item-info">
              <span className="config-server-name">{server.name}</span>
              <span className="config-server-command">{server.command} {server.args.join(' ')}</span>
            </div>
            <button className="config-server-edit" onClick={() => onEdit(idx)}>
              {editIdx === idx ? '收起' : '目标'}
            </button>
            <button className="config-server-remove" onClick={() => onRemove(idx)}>×</button>
          </div>
          {editIdx === idx && (
            <div className="config-server-targets">
              <span className="config-server-targets-label">目标 Agent：</span>
              {ALL_AGENTS.map((a) => (
                <label key={a.value} className="config-server-target-check">
                  <input
                    type="checkbox"
                    checked={server.targetAgents.includes(a.value)}
                    onChange={(e) => onUpdateTarget(idx, a.value, e.target.checked)}
                  />
                  <span>{a.label}</span>
                </label>
              ))}
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

export { ALL_AGENTS };
