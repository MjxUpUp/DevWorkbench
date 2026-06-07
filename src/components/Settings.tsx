import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getVersion } from '@tauri-apps/api/app';
import { check } from '@tauri-apps/plugin-updater';
import { relaunch } from '@tauri-apps/plugin-process';
import { IconX } from './Icons';
import type { AppSettings, ToolStatus } from '../types';

type UpdateStatus = 'idle' | 'checking' | 'up-to-date' | 'available' | 'downloading' | 'ready' | 'error';

interface SettingsProps {
  tools: ToolStatus[];
  onClose: () => void;
}

export function Settings({ tools, onClose }: SettingsProps) {
  const [settings, setSettings] = useState<AppSettings>({
    scan_directories: [],
    tool_paths: {},
  });
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
      const update = await check();

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

  const removeScanDir = (dir: string) => {
    save({ ...settings, scan_directories: settings.scan_directories.filter(d => d !== dir) });
  };

  const setToolPath = (tool: string, path: string) => {
    save({ ...settings, tool_paths: { ...settings.tool_paths, [tool]: path } });
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
              <button onClick={addScanDir}>添加</button>
            </div>
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
