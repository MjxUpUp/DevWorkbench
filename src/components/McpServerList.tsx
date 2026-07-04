import type { McpServerConfig } from '../types';
import { Button } from './ui/Button/Button';

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
        </div>
      ))}
    </div>
  );
}
