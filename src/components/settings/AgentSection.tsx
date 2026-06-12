import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAgentStore } from '../../stores/agentStore';
import type { AppSettings, ToolStatus, TerminalInfo } from '../../types';

const NON_AGENT_TOOLS = ['code', 'git'];

export function AgentSection() {
  const agents = useAgentStore(s => s.agents);
  const [settings, setSettings] = useState<AppSettings>({
    scan_directories: [],
    tool_paths: {},
    theme: 'light',
    preferred_terminal: '',
    cli_flags: {},
  });
  const [terminals, setTerminals] = useState<TerminalInfo[]>([]);
  const [tools, setTools] = useState<ToolStatus[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<AppSettings>('load_settings')
      .then(data => { setSettings(data); setError(null); })
      .catch(e => setError(`加载设置失败: ${e}`));
  }, []);

  useEffect(() => {
    invoke<ToolStatus[]>('detect_tools')
      .then(setTools)
      .catch(() => {});
  }, []);

  useEffect(() => {
    invoke<TerminalInfo[]>('detect_terminals')
      .then(setTerminals)
      .catch(() => {});
  }, []);

  const save = async (updated: AppSettings) => {
    try {
      await invoke('save_settings', { settings: updated });
      setSettings(updated);
      setError(null);
    } catch (e) {
      setError(`保存设置失败: ${e}`);
    }
  };

  const setToolPath = (tool: string, path: string) => {
    save({ ...settings, tool_paths: { ...settings.tool_paths, [tool]: path } });
  };

  const setCliFlag = (tool: string, flag: string) => {
    save({ ...settings, cli_flags: { ...settings.cli_flags, [tool]: flag } });
  };

  const setPreferredTerminal = (id: string) => {
    save({ ...settings, preferred_terminal: id });
  };

  // Build unified tool list
  const agentEntries = agents.map(a => ({
    key: a.commandName,
    label: a.displayName,
    installed: a.installed,
    path: a.path,
    isAgent: true,
  }));

  const nonAgentEntries = tools
    .filter(t => NON_AGENT_TOOLS.includes(t.name))
    .map(t => ({
      key: t.name,
      label: t.name === 'code' ? 'VS Code' : t.name,
      installed: t.installed,
      path: t.path,
      isAgent: false,
    }));

  const allToolEntries = [...agentEntries, ...nonAgentEntries];

  // CLI flags
  const cliEntries = agents.map(a => ({
    key: a.commandName,
    label: a.displayName,
    hint: a.agentType === 'claude_code' ? '--dangerously-skip-permissions' : '',
  }));

  return (
    <>
      {error && <div className="error-banner" style={{ margin: 0, marginBottom: 16 }}>{error}</div>}

      <div className="settings-section">
        <h3 className="settings-section-title">工具状态</h3>
        <div className="settings-tools">
          {allToolEntries.map(entry => (
            <div key={entry.key} className="settings-tool-row">
              <span className={`status-dot ${entry.installed ? 'installed' : ''}`} />
              <span className="tool-name">{entry.label}</span>
              <span className="tool-status">{entry.installed ? `✓ ${entry.path ?? ''}` : '未安装'}</span>
              <input
                className="tool-path-input"
                value={settings.tool_paths[entry.key] || ''}
                onChange={e => setToolPath(entry.key, e.target.value)}
                placeholder="自定义路径（可选）"
              />
            </div>
          ))}
        </div>
      </div>

      <div className="settings-section">
        <h3 className="settings-section-title">终端偏好</h3>
        <div className="settings-terminal">
          {terminals.length === 0 && <span className="terminal-hint">检测中...</span>}
          {terminals.map(t => (
            <button
              key={t.id}
              className={`terminal-option ${settings.preferred_terminal === t.id ? 'active' : ''} ${!t.available ? 'unavailable' : ''}`}
              onClick={() => t.available && setPreferredTerminal(t.id)}
              disabled={!t.available}
              title={t.available ? `使用 ${t.label}` : `${t.label} 未安装`}
            >
              <span className={`status-dot ${t.available ? 'installed' : ''}`} />
              <span className="terminal-name">{t.label}</span>
              {!t.available && <span className="terminal-unavail">未安装</span>}
            </button>
          ))}
          {settings.preferred_terminal && (
            <button
              className="terminal-option reset"
              onClick={() => setPreferredTerminal('')}
              title="恢复自动检测"
            >
              自动检测
            </button>
          )}
        </div>
      </div>

      <div className="settings-section">
        <h3 className="settings-section-title">CLI 启动参数</h3>
        <div className="settings-cli-flags">
          {cliEntries.map(t => (
            <div key={t.key} className="cli-flag-row">
              <span className="cli-tool-name">{t.label}</span>
              <input
                className="cli-flag-input"
                value={settings.cli_flags[t.key] || ''}
                onChange={e => setCliFlag(t.key, e.target.value)}
                placeholder={t.hint || '自定义启动参数'}
              />
            </div>
          ))}
          <span className="cli-flags-hint">如 Claude Code 的 --dangerously-skip-permissions、Codex 的 --full-auto 等</span>
        </div>
      </div>
    </>
  );
}
