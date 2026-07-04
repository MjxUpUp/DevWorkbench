import type { McpServerConfig, AgentType } from '../types';
import { Button } from './ui/Button/Button';

// ALL_AGENTS 仍 export — McpSection 用作 targetAgents 默认值（schema 兼容保留）。
// UI 渲染已隐藏（CLI 路径退役）；但 server 配置默认值保留所有 CLI agent 以便
// 老配置 round-trip 不丢值。
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
}

export function McpServerList({ servers, editIdx, onToggle, onRemove, onEdit }: McpServerListProps) {
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
            <Button variant="ghost" size="sm" onClick={() => onEdit(idx)}>
              {editIdx === idx ? '收起' : '目标'}
            </Button>
            <Button variant="dangerGhost" size="sm" onClick={() => onRemove(idx)} aria-label="移除">×</Button>
          </div>
          {editIdx === idx && (
            <div className="config-server-targets" role="note">
              <span className="config-server-targets-label">目标 Agent 选择器已下架：</span>
              <span style={{ color: 'var(--text-tertiary)', fontSize: 'var(--text-xs)' }}>
                MCP server 默认对所有 agent 生效（<strong>新建 server</strong> 默认值）。
                <strong>已存配置</strong>的 <code>targetAgents</code> 限制仍生效 —— 如需调整请编辑项目 <code>.mcp.toml</code>。
              </span>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

export { ALL_AGENTS };
