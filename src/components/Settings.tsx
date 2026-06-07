import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { open } from '@tauri-apps/plugin-dialog';
import { IconX } from './Icons';
import type { AppSettings, ToolStatus, TerminalInfo } from '../types';

type UpdateStatus = 'idle' | 'checking' | 'up-to-date' | 'available' | 'downloading' | 'ready' | 'error';

interface SettingsProps {
  tools: ToolStatus[];
  theme: string;
  onThemeChange: (theme: string) => void;
  onClose: () => void;
}

const THEMES = [
  { key: 'obsidian', label: '黑曜石', dot: 'obsidian' },
  { key: 'midnight', label: '午夜蓝', dot: 'midnight' },
  { key: 'ember', label: '琥珀', dot: 'ember' },
  { key: 'rose', label: '玫瑰', dot: 'rose' },
  { key: 'nord', label: '极光', dot: 'nord' },
  { key: 'daylight', label: '日光', dot: 'daylight' },
  { key: 'paper', label: '纸白', dot: 'paper' },
  { key: 'mint', label: '薄荷', dot: 'mint' },
] as const;

const CLI_TOOLS = [
  { key: 'claude', label: 'Claude Code', hint: '--dangerously-skip-permissions' },
  { key: 'codex', label: 'Codex', hint: '--full-auto' },
  { key: 'pi', label: 'Pi', hint: '' },
] as const;

export function Settings({ tools, theme, onThemeChange, onClose }: SettingsProps) {
  const [settings, setSettings] = useState<AppSettings>({
    scan_directories: [],
    tool_paths: {},
    theme: 'obsidian',
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

  useEffect(() => {
    getVersion().then(v => setAppVersion(v));
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

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal" onClick={e => e.stopPropagation()}>
        <div className="modal-header">
          <h2>设置</h2>
          <button className="modal-close" onClick={onClose}><IconX size={16} /></button>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <div className="modal-body">
          <h3>工具状态</h3>
          <div className="settings-tools">
            {tools.map(tool => (
              <div key={tool.name} className="settings-tool-row">
                <span className={`status-dot ${tool.installed ? 'installed' : ''}`} />
                <span className="tool-name">{tool.name}</span>
                <span className="tool-status">{tool.installed ? `✓ ${tool.path}` : '未安装'}</span>
                <input
                  className="tool-path-input"
                  value={settings.tool_paths[tool.name] || ''}
                  onChange={e => setToolPath(tool.name, e.target.value)}
                  placeholder="自定义路径（可选）"
                />
              </div>
            ))}
          </div>

          <h3>终端偏好</h3>
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

          <h3>CLI 启动参数</h3>
          <div className="settings-cli-flags">
            {CLI_TOOLS.map(t => (
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

          <h3>扫描目录</h3>
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

          <h3>主题</h3>
          <div className="theme-picker">
            {THEMES.map(t => (
              <button
                key={t.key}
                className={`theme-swatch ${theme === t.key ? 'active' : ''}`}
                onClick={() => onThemeChange(t.key)}
              >
                <span className={`theme-swatch-dot ${t.dot}`} />
                {t.label}
              </button>
            ))}
          </div>

          <h3>关于</h3>
          <div className="settings-about">
            <span className="about-version">一目了然 v{appVersion}</span>
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
      </div>
    </div>
  );
}
