import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { open } from '@tauri-apps/plugin-dialog';
import { IconX } from './Icons';
import type { AppSettings, ToolStatus, TerminalInfo, AgentInfo } from '../types';

type UpdateStatus = 'idle' | 'checking' | 'up-to-date' | 'available' | 'downloading' | 'ready' | 'error';

interface SettingsProps {
  tools: ToolStatus[];
  agents: AgentInfo[];
  onClose: () => void;
}

type SettingsSection = 'general' | 'appearance' | 'agent' | 'providers' | 'mcp' | 'hooks' | 'skills' | 'memory' | 'about';

const SECTIONS: { id: SettingsSection; label: string; icon: string }[] = [
  { id: 'general', label: '通用', icon: '⚙️' },
  { id: 'appearance', label: '外观', icon: '🎨' },
  { id: 'agent', label: 'Agent 管理', icon: '🤖' },
  { id: 'providers', label: '模型供应商', icon: '🧠' },
  { id: 'mcp', label: 'MCP 服务器', icon: '🔌' },
  { id: 'hooks', label: 'Hooks', icon: '🪝' },
  { id: 'skills', label: '技能', icon: '⚡' },
  { id: 'memory', label: '记忆', icon: '🧠' },
  { id: 'about', label: '关于', icon: 'ℹ️' },
];

// Non-agent tools that are shown in settings (IDE, git)
const NON_AGENT_TOOLS = ['code', 'git'];

export function Settings({ tools, agents, onClose }: SettingsProps) {
  const [activeSection, setActiveSection] = useState<SettingsSection>('general');
  const [settings, setSettings] = useState<AppSettings>({
    scan_directories: [],
    tool_paths: {},
    theme: 'light',
    preferred_terminal: '',
    cli_flags: {},
  });
  const [terminals, setTerminals] = useState<TerminalInfo[]>([]);
  const [newScanDir, setNewScanDir] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [updateStatus, setUpdateStatus] = useState<UpdateStatus>('idle');
  const [updateVersion, setUpdateVersion] = useState('');
  const [downloadProgress, setDownloadProgress] = useState(0);
  const [appVersion, setAppVersion] = useState('...');
  const [currentTheme, setCurrentTheme] = useState<'light' | 'dark'>('light');

  useEffect(() => {
    getVersion().then(v => setAppVersion(v));
  }, []);

  useEffect(() => {
    // Detect current theme
    const theme = document.documentElement.getAttribute('data-theme');
    setCurrentTheme(theme === 'dark' ? 'dark' : 'light');
  }, []);

  useEffect(() => {
    invoke<AppSettings>('load_settings')
      .then(data => { setSettings(data); setError(null); })
      .catch(e => setError(`加载设置失败: ${e}`));
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

  const setTheme = (theme: 'light' | 'dark') => {
    setCurrentTheme(theme);
    if (theme === 'dark') {
      document.documentElement.setAttribute('data-theme', 'dark');
    } else {
      document.documentElement.removeAttribute('data-theme');
    }
    save({ ...settings, theme });
  };

  const handleCheckUpdate = async () => {
    try {
      setUpdateStatus('checking');
      const update = await Promise.race([
        check(),
        new Promise<never>((_, reject) => setTimeout(() => reject(new Error('检查更新超时')), 15000)),
      ]);

      if (!update) {
        setUpdateStatus('up-to-date');
        return;
      }

      setUpdateVersion(update.version);
      setUpdateStatus('downloading');

      let downloaded = 0;
      let contentLength = 0;
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case 'Started':
            contentLength = event.data.contentLength ?? 0;
            break;
          case 'Progress':
            downloaded += event.data.chunkLength;
            if (contentLength > 0) {
              setDownloadProgress(Math.round((downloaded / contentLength) * 100));
            }
            break;
          case 'Finished':
            break;
        }
      });

      setUpdateStatus('ready');
    } catch (e) {
      setUpdateStatus('error');
      console.warn('Update check failed:', e);
    }
  };

  const handleRelaunch = async () => {
    await relaunch();
  };

  const addScanDir = () => {
    if (!newScanDir || settings.scan_directories.includes(newScanDir)) return;
    save({ ...settings, scan_directories: [...settings.scan_directories, newScanDir] });
    setNewScanDir('');
  };

  const pickScanDir = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected && typeof selected === 'string') {
      setNewScanDir(selected);
    }
  };

  const removeScanDir = (dir: string) => {
    save({ ...settings, scan_directories: settings.scan_directories.filter(d => d !== dir) });
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

  const getUpdateText = () => {
    switch (updateStatus) {
      case 'checking': return '正在检查更新...';
      case 'up-to-date': return '已是最新版本';
      case 'available': return `发现新版本 ${updateVersion}`;
      case 'downloading': return `正在下载更新 ${downloadProgress}%`;
      case 'ready': return `新版本 ${updateVersion} 已就绪`;
      case 'error': return '检查更新失败';
      default: return '';
    }
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

  // Render section content
  const renderSection = () => {
    switch (activeSection) {
      case 'general':
        return (
          <>
            <div className="settings-section">
              <h3 className="settings-section-title">扫描目录</h3>
              <div className="settings-scan-dirs">
                {settings.scan_directories.map(dir => (
                  <div key={dir} className="scan-dir-row">
                    <span>{dir}</span>
                    <button onClick={() => removeScanDir(dir)}><IconX size={14} /></button>
                  </div>
                ))}
                <div className="input-row">
                  <input value={newScanDir} onChange={e => setNewScanDir(e.target.value)} placeholder="添加扫描目录路径" />
                  <button onClick={pickScanDir}>选择</button>
                  <button onClick={addScanDir}>添加</button>
                </div>
              </div>
            </div>
          </>
        );

      case 'appearance':
        return (
          <div className="settings-section">
            <h3 className="settings-section-title">主题</h3>
            <p className="settings-section-desc">选择 Dev Workbench 的外观主题</p>
            <div className="theme-selector">
              <button
                className={`theme-option ${currentTheme === 'light' ? 'active' : ''}`}
                onClick={() => setTheme('light')}
              >
                <div className="theme-option-preview light" />
                <span className="theme-option-label">浅色</span>
              </button>
              <button
                className={`theme-option ${currentTheme === 'dark' ? 'active' : ''}`}
                onClick={() => setTheme('dark')}
              >
                <div className="theme-option-preview dark" />
                <span className="theme-option-label">深色</span>
              </button>
            </div>
          </div>
        );

      case 'agent':
        return (
          <>
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

      case 'providers':
        return (
          <div className="settings-section">
            <h3 className="settings-section-title">模型供应商</h3>
            <p className="settings-section-desc">配置 AI 模型供应商和 API Key，支持多供应商切换</p>
            <div className="provider-list">
              {[
                { name: 'Z.AI (GLM)', status: '预置', color: 'var(--accent)' },
                { name: 'BigModel', status: '预置', color: 'var(--accent)' },
                { name: 'Anthropic', status: '自定义', color: 'var(--text-muted)' },
                { name: 'DeepSeek', status: '自定义', color: 'var(--text-muted)' },
                { name: 'OpenRouter', status: '自定义', color: 'var(--text-muted)' },
              ].map(provider => (
                <div key={provider.name} className="provider-card">
                  <div className="provider-card-header">
                    <span className="provider-name">{provider.name}</span>
                    <span className="provider-status disconnected">{provider.status}</span>
                  </div>
                  <div className="provider-fields">
                    <div className="provider-field">
                      <span className="provider-field-label">接口地址</span>
                      <input className="provider-field-input" placeholder="https://api.example.com/v1" />
                    </div>
                    <div className="provider-field">
                      <span className="provider-field-label">API Key</span>
                      <input className="provider-field-input" type="password" placeholder="sk-..." />
                    </div>
                  </div>
                  <div className="provider-actions">
                    <button className="provider-btn primary">保存</button>
                    <button className="provider-btn secondary">测试连接</button>
                  </div>
                </div>
              ))}
            </div>
          </div>
        );

      case 'mcp':
        return (
          <div className="settings-section">
            <h3 className="settings-section-title">MCP 服务器</h3>
            <p className="settings-section-desc">通过配置中心管理 MCP 服务器配置</p>
            <button className="primary-btn" onClick={() => { onClose(); /* Will be opened via store */ }}>
              打开配置中心
            </button>
          </div>
        );

      case 'hooks':
        return (
          <div className="settings-section">
            <h3 className="settings-section-title">Hooks 配置</h3>
            <p className="settings-section-desc">Forge Hooks — 在特定事件触发时自动执行命令</p>
            <div style={{ color: 'var(--text-secondary)', fontSize: 13 }}>
              <div className="hook-item">
                <span className="hook-name">pre-commit</span>
                <span className="hook-command">forge quality gate</span>
              </div>
              <div className="hook-item">
                <span className="hook-name">post-session</span>
                <span className="hook-command">forge collect</span>
              </div>
            </div>
          </div>
        );

      case 'skills':
        return (
          <div className="settings-section">
            <h3 className="settings-section-title">已注册技能</h3>
            <p className="settings-section-desc">Forge 管道和技能管理</p>
            <div className="skill-list">
              {[
                { name: '/forge-pipeline', source: 'Forge', desc: '运行项目级质量管道' },
                { name: '/forge-quality', source: 'Forge', desc: '查看完整质量协议' },
                { name: '/plan', source: '内置', desc: '计划模式' },
                { name: '/review', source: '内置', desc: '代码审查' },
                { name: '/test', source: '内置', desc: '运行测试' },
              ].map(skill => (
                <div key={skill.name} className="skill-item">
                  <span className="skill-name">{skill.name}</span>
                  <span className="skill-source">{skill.source}</span>
                  <span className="skill-desc">{skill.desc}</span>
                </div>
              ))}
            </div>
          </div>
        );

      case 'memory':
        return (
          <div className="settings-section">
            <h3 className="settings-section-title">项目记忆</h3>
            <p className="settings-section-desc">管理 Dev Workbench 的项目记忆和知识库</p>
            <div style={{ color: 'var(--text-secondary)', fontSize: 13 }}>
              记忆存储在项目 .claude/ 目录中，由 Agent 自动管理
            </div>
          </div>
        );

      case 'about':
        return (
          <div className="settings-section">
            <h3 className="settings-section-title">关于</h3>
            <div className="settings-about">
              <span className="about-version">Dev Workbench v{appVersion}</span>
              <div className="update-row">
                {updateStatus === 'idle' || updateStatus === 'up-to-date' || updateStatus === 'error' ? (
                  <button className="update-btn" onClick={handleCheckUpdate}>
                    检查更新
                  </button>
                ) : updateStatus === 'ready' ? (
                  <button className="update-btn update-restart" onClick={handleRelaunch}>
                    重启以完成更新
                  </button>
                ) : (
                  <button className="update-btn" disabled>
                    {getUpdateText()}
                  </button>
                )}
                {updateStatus !== 'idle' && updateStatus !== 'ready' && (
                  <span className={`update-status ${updateStatus === 'error' ? 'error' : ''}`}>
                    {getUpdateText()}
                  </span>
                )}
                {updateStatus === 'up-to-date' && (
                  <span className="update-status success">✓ 已是最新版本</span>
                )}
              </div>
            </div>
          </div>
        );
    }
  };

  return (
    <div className="settings-page-overlay">
      {/* Left navigation */}
      <div className="settings-page-nav">
        <div className="settings-page-nav-header">
          <h2>设置</h2>
          <button className="settings-page-close" onClick={onClose}><IconX size={16} /></button>
        </div>
        {SECTIONS.map(section => (
          <button
            key={section.id}
            className={`settings-nav-item ${activeSection === section.id ? 'active' : ''}`}
            onClick={() => setActiveSection(section.id)}
          >
            <span className="settings-nav-icon">{section.icon}</span>
            {section.label}
          </button>
        ))}
      </div>

      {/* Right content */}
      <div className="settings-page-content">
        {error && <div className="error-banner" style={{ margin: 0, marginBottom: 16 }}>{error}</div>}
        {renderSection()}
      </div>
    </div>
  );
}
