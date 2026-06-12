import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import { IconX } from '../Icons';
import type { AppSettings } from '../../types';

export function GeneralSection() {
  const [settings, setSettings] = useState<AppSettings>({
    scan_directories: [],
    tool_paths: {},
    theme: 'light',
    preferred_terminal: '',
    cli_flags: {},
  });
  const [newScanDir, setNewScanDir] = useState('');
  const [error, setError] = useState<string | null>(null);

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

  return (
    <>
      {error && <div className="error-banner" style={{ margin: 0, marginBottom: 16 }}>{error}</div>}
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
}
