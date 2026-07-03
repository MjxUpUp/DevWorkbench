import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAgentStore } from '../../stores/agentStore';
import { useSettingsStore } from '../../stores/settingsStore';
import { useDebouncedCallback } from '../../hooks/useDebouncedCallback';
import type { AppSettings, ToolStatus, TerminalInfo } from '../../types';

const NON_AGENT_TOOLS = ['code', 'git'];

/**
 * AgentSection — 设置页「智能体工具」分区。
 *
 * CLI 默认执行路径已退役（起底重构：ReactKernel 唯一执行路径），但下列工具配置仍有
 * 真实消费链，故保留：
 *  - 工具状态（detect_tools + tool_paths 自定义路径）：OpaqueAgent 桥（workflow 节点
 *    接外部 CLI）经 discovery.rs 用 tool_paths 定位 claude/codex 二进制；editor.rs 用
 *    它打开 VS Code；code/git 检测服务项目页 ToolButton。
 *  - 终端偏好（preferred_terminal）：open_terminal 经 terminal.rs:163 读它选终端 app，
 *    GitPanel「打开终端做 git commit」仍走此路径——与 CLI agent 执行无关。
 *
 * 已移除：「CLI 启动参数」（cli_flags）段——其唯一消费方是 launchTool 在外部终端启动
 * CLI agent（claude/pi/codex）时拼 --dangerously-skip-permissions 等启动 flag。CLI 取消
 * 后该外部终端执行路径退役，cli_flags 不再有配置意义。launchTool 的兼容性读取（永远
 * 空）保留，AppSettings.cli_flags 字段 + DB 列保留以免迁移，只是设置页不再暴露编辑。
 */
export function AgentSection() {
  const agents = useAgentStore(s => s.agents);
  const settings = useSettingsStore((s) => s.settings);
  const saveSettings = useSettingsStore((s) => s.saveSettings);
  const error = useSettingsStore((s) => s.error);

  const [terminals, setTerminals] = useState<TerminalInfo[]>([]);
  const [tools, setTools] = useState<ToolStatus[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      invoke<ToolStatus[]>('detect_tools').catch(() => [] as ToolStatus[]),
      invoke<TerminalInfo[]>('detect_terminals').catch(() => [] as TerminalInfo[]),
    ]).then(([t, tm]) => {
      if (cancelled) return;
      setTools(Array.isArray(t) ? t : []);
      setTerminals(Array.isArray(tm) ? tm : []);
      setLoading(false);
    });
    return () => { cancelled = true; };
  }, []);

  // Debounce saves so a typing burst is one IPC write, not one per keystroke —
  // without this, fast typing floods the backend with concurrent save_settings
  // calls racing into the same row (IPC backlog + last-writer-lost).
  const debouncedSave = useDebouncedCallback(
    (patch: Partial<AppSettings>) => { void saveSettings(patch); },
    300,
  );

  const setToolPath = (tool: string, path: string) => {
    debouncedSave({ tool_paths: { ...settings.tool_paths, [tool]: path } });
  };

  const setPreferredTerminal = (id: string) => {
    saveSettings({ preferred_terminal: id });
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

  return (
    <>
      {error && <div className="error-banner" style={{ margin: 0, marginBottom: 16 }}>{error}</div>}
      {loading && <div className="config-center-loading" style={{ padding: 40, textAlign: 'center' }}>检测工具与终端中...</div>}

      {/* Tool status — zcode card list */}
      <div className="settings-section">
        <h3 className="settings-section-title">工具状态</h3>
        <p className="settings-section-desc">检测系统中已安装的 Agent 和工具。</p>
        <div className="settings-card-list">
          {allToolEntries.map(entry => (
            <div key={entry.key} className="settings-tool-card">
              <div className="settings-tool-card-header">
                <div className="settings-tool-card-info">
                  <span className="settings-tool-card-name">{entry.label}</span>
                  <span className={`settings-tool-card-status ${entry.installed ? 'installed' : 'not-installed'}`}>
                    {entry.installed ? '已安装' : '未安装'}
                  </span>
                </div>
                {entry.installed && entry.path && (
                  <span className="settings-tool-card-path">{entry.path}</span>
                )}
              </div>
              <div className="settings-tool-card-actions">
                <input
                  className="settings-row-input settings-row-input-sm"
                  value={settings.tool_paths[entry.key] || ''}
                  onChange={e => setToolPath(entry.key, e.target.value)}
                  placeholder="自定义路径（可选）"
                />
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* Terminal preference — zcode settings rows */}
      <div className="settings-section">
        <h3 className="settings-section-title">终端偏好</h3>
        <p className="settings-section-desc">选择 Agent 任务使用的终端。</p>

        <div className="settings-card-list">
          {terminals.length === 0 && (
            <div className="settings-tool-card">
              <span className="settings-tool-card-name" style={{ color: 'var(--text-tertiary)' }}>检测中...</span>
            </div>
          )}
          {terminals.map(t => (
            <div
              key={t.id}
              className={`settings-tool-card selectable ${settings.preferred_terminal === t.id ? 'selected' : ''} ${!t.available ? 'disabled' : ''}`}
              onClick={() => t.available && setPreferredTerminal(t.id)}
            >
              <div className="settings-tool-card-header">
                <div className="settings-tool-card-info">
                  <span className="settings-tool-card-name">{t.label}</span>
                  {!t.available && <span className="settings-tool-card-status not-installed">未安装</span>}
                  {t.available && <span className="settings-tool-card-status installed">可用</span>}
                </div>
                {settings.preferred_terminal === t.id && (
                  <span className="settings-tool-card-badge">默认</span>
                )}
              </div>
            </div>
          ))}
          {settings.preferred_terminal && (
            <button
              className="settings-tool-card selectable"
              onClick={() => setPreferredTerminal('')}
              style={{ borderStyle: 'dashed', justifyContent: 'center' }}
            >
              <span className="settings-tool-card-name" style={{ color: 'var(--text-tertiary)' }}>自动检测</span>
            </button>
          )}
        </div>
      </div>
    </>
  );
}
