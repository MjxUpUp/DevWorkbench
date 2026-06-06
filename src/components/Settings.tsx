import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { AppSettings, ToolStatus } from '../types';

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

  useEffect(() => {
    invoke<AppSettings>('load_settings')
      .then(setSettings)
      .catch(console.error);
  }, []);

  const save = async (updated: AppSettings) => {
    await invoke('save_settings', { settings: updated });
    setSettings(updated);
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

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal" onClick={e => e.stopPropagation()}>
        <div className="modal-header">
          <h2>设置</h2>
          <button className="modal-close" onClick={onClose}>✕</button>
        </div>

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
                <button onClick={() => removeScanDir(dir)}>✕</button>
              </div>
            ))}
            <div className="input-row">
              <input value={newScanDir} onChange={e => setNewScanDir(e.target.value)} placeholder="添加扫描目录路径" />
              <button onClick={addScanDir}>添加</button>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
