import { useState, useEffect, useCallback } from 'react';
import { useNavigationStore } from '../../stores/navigationStore';
import { useConfigStore } from '../../stores/configStore';
import { useAgentStore } from '../../stores/agentStore';
import { McpServerList, ALL_AGENTS } from '../McpServerList';
import type { McpServerConfig, AgentType } from '../../types';

const EMPTY_SERVER: Omit<McpServerConfig, 'name'> = {
  command: '',
  args: [],
  env: {},
  enabled: true,
  targetAgents: ALL_AGENTS.map((a) => a.value),
};

export function McpSection() {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const agents = useAgentStore((s) => s.agents);
  const { mcpConfig, loading, loadConfig, saveConfig, applyConfig } = useConfigStore();

  const [servers, setServers] = useState<McpServerConfig[]>([]);
  const [newName, setNewName] = useState('');
  const [newCommand, setNewCommand] = useState('');
  const [editIdx, setEditIdx] = useState<number | null>(null);
  const [applyResult, setApplyResult] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const installedAgents = agents.filter((a) => a.installed);

  // Load config when project is selected
  useEffect(() => {
    if (activeProject) {
      loadConfig(activeProject.path);
    }
  }, [activeProject, loadConfig]);

  // Sync local state when config loads
  useEffect(() => {
    if (mcpConfig) {
      setServers(mcpConfig.servers);
    }
  }, [mcpConfig]);

  const handleSave = useCallback(async () => {
    if (!activeProject) return;
    setError(null);
    try {
      await saveConfig(activeProject.path, { servers });
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [activeProject, servers, saveConfig]);

  const handleApply = useCallback(async () => {
    if (!activeProject) return;
    setError(null);
    try {
      await saveConfig(activeProject.path, { servers });
      const result = await applyConfig(activeProject.path, { servers });
      setApplyResult(result);
    } catch (e: unknown) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [activeProject, servers, saveConfig, applyConfig]);

  const addServer = () => {
    if (!newName.trim() || !newCommand.trim()) return;
    if (servers.some((s) => s.name === newName.trim())) {
      setError('Server name already exists');
      return;
    }
    setServers([...servers, {
      ...EMPTY_SERVER,
      name: newName.trim(),
      command: newCommand.trim(),
    }]);
    setNewName('');
    setNewCommand('');
    setError(null);
  };

  const removeServer = (idx: number) => {
    setServers(servers.filter((_, i) => i !== idx));
  };

  const toggleServer = (idx: number) => {
    setServers(servers.map((s, i) => i === idx ? { ...s, enabled: !s.enabled } : s));
  };

  const updateServerTarget = (idx: number, agent: AgentType, checked: boolean) => {
    setServers(servers.map((s, i) => {
      if (i !== idx) return s;
      const targets = checked
        ? [...s.targetAgents, agent]
        : s.targetAgents.filter((a) => a !== agent);
      return { ...s, targetAgents: targets.length > 0 ? targets : [agent] };
    }));
  };

  if (!activeProject) {
    return (
      <div className="settings-section">
        <h3 className="settings-section-title">MCP 服务器</h3>
        <p className="settings-section-desc">请先从左侧选择一个项目，然后配置该项目的 MCP 服务器</p>
      </div>
    );
  }

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">MCP 服务器</h3>
      <p className="settings-section-desc">管理项目 <code>{activeProject.name}</code> 的 MCP 服务器配置</p>

      {loading && <div className="config-center-loading">加载中...</div>}
      {error && <div className="config-center-error">{error}</div>}

      {/* Server list */}
      <McpServerList
        servers={servers}
        editIdx={editIdx}
        onToggle={toggleServer}
        onRemove={removeServer}
        onEdit={(idx) => setEditIdx(editIdx === idx ? null : idx)}
        onUpdateTarget={updateServerTarget}
      />

      {/* Add server form */}
      <div className="config-server-add">
        <input
          type="text"
          placeholder="Server 名称 (如 filesystem)"
          value={newName}
          onChange={(e) => setNewName(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') addServer(); }}
          className="config-server-input"
        />
        <input
          type="text"
          placeholder="命令 (如 npx -y @mcp/server-filesystem /tmp)"
          value={newCommand}
          onChange={(e) => setNewCommand(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') addServer(); }}
          className="config-server-input config-server-input-wide"
        />
        <button className="config-server-add-btn" onClick={addServer} disabled={!newName.trim() || !newCommand.trim()}>
          添加
        </button>
      </div>

      {/* Actions */}
      <div className="config-server-actions">
        <button className="config-server-save-btn" onClick={handleSave}>
          保存配置
        </button>
        <button className="config-server-apply-btn" onClick={handleApply}>
          应用到项目
        </button>
      </div>

      {/* Apply result */}
      {applyResult && (
        <div className="config-apply-result">
          <p>已生成以下配置文件：</p>
          <ul>
            {applyResult.map((r, i) => (
              <li key={i}><code>{r}</code></li>
            ))}
          </ul>
        </div>
      )}

      {/* Installed agents info */}
      {installedAgents.length > 0 && (
        <div className="config-agents-info">
          <span className="config-agents-label">已安装的 Agent：</span>
          {installedAgents.map((a) => (
            <span key={a.agentType} className="config-agent-badge">{a.displayName}</span>
          ))}
        </div>
      )}
    </div>
  );
}
