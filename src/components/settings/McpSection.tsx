import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useNavigationStore } from '../../stores/navigationStore';
import { useConfigStore } from '../../stores/configStore';
import { Button } from '../ui/Button/Button';
import { McpServerList } from '../McpServerList';
import { McpRuntimePanel } from './McpRuntimePanel';
import type { McpServerConfig } from '../../types';

const EMPTY_SERVER: Omit<McpServerConfig, 'name'> = {
  command: '',
  args: [],
  env: {},
  enabled: true,
};

export function McpSection() {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const { mcpConfig, loading, loadConfig, saveConfig, applyConfig } = useConfigStore();

  const [servers, setServers] = useState<McpServerConfig[]>([]);
  const [newName, setNewName] = useState('');
  const [newCommand, setNewCommand] = useState('');
  const [editIdx, setEditIdx] = useState<number | null>(null);
  const [applyResult, setApplyResult] = useState<string[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Refs to detect unsaved local edits on project switch without widening the
  // load effect's dep array. `serversRef` mirrors the live local edits;
  // `lastLoadedRef` is the config most recently pushed into local state. They
  // differ by reference exactly when the user has edited but not saved.
  const serversRef = useRef(servers);
  serversRef.current = servers;
  const lastLoadedRef = useRef<McpServerConfig[] | null>(null);

  // Load config when project is selected. Guard against silently discarding
  // unsaved edits: if local servers diverge (by reference) from what was
  // loaded, confirm before overwriting them with the new project's config.
  useEffect(() => {
    if (!activeProject) return;
    if (
      lastLoadedRef.current !== null &&
      serversRef.current !== lastLoadedRef.current
    ) {
      if (!window.confirm('当前项目有未保存的 MCP 配置更改，切换项目将丢弃这些更改。是否继续？')) {
        return;
      }
    }
    loadConfig(activeProject.path);
  }, [activeProject, loadConfig]);

  // Sync local state when config loads
  useEffect(() => {
    if (mcpConfig) {
      setServers(mcpConfig.servers);
      lastLoadedRef.current = mcpConfig.servers;
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
      // D5: reconnect enabled servers into the live McpRegistry so agents
      // spawned after this can use them (previously the registry stayed empty
      // until a manual mcp_install_preset). Best-effort: a failing server is
      // logged + skipped server-side, and the config was already applied above,
      // so a reconnect error here must NOT mask the successful apply.
      try {
        await invoke('mcp_load_enabled', { projectPath: activeProject.path });
      } catch {
        // live reconnect is best-effort — swallow, apply already succeeded
      }
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

  // target_agents / targetAgents 字段已彻底删除（schema + 后端过滤 + UI）。
  // 老 .mcp.toml 中残留 `target_agents = [...]` 行被 parse_mcp_config 优雅忽略。
  // 见 McpServerList.tsx（折叠后不再有 "目标 Agent" 区块）。

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
        <Button variant="primary" onClick={addServer} disabled={!newName.trim() || !newCommand.trim()}>
          添加
        </Button>
      </div>

      {/* Actions */}
      <div className="config-server-actions">
        <Button variant="secondary" onClick={handleSave}>
          保存配置
        </Button>
        <Button variant="primary" onClick={handleApply}>
          应用到项目
        </Button>
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

      {/* B3: 运行时管理（即时连接/断开/工具试跑） */}
      <McpRuntimePanel servers={servers} />
    </div>
  );
}
