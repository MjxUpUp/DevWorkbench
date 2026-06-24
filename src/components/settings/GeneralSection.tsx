import { useState } from 'react';
import { open } from '@tauri-apps/plugin-dialog';
import { IconX, IconFolderOpen } from '../Icons';
import { useSettingsStore } from '../../stores/settingsStore';
import { Button } from '../ui/Button/Button';

export function GeneralSection() {
  const settings = useSettingsStore((s) => s.settings);
  const saveSettings = useSettingsStore((s) => s.saveSettings);
  const error = useSettingsStore((s) => s.error);

  const [newScanDir, setNewScanDir] = useState('');

  const addScanDir = () => {
    if (!newScanDir || settings.scan_directories.includes(newScanDir)) return;
    saveSettings({ scan_directories: [...settings.scan_directories, newScanDir] });
    setNewScanDir('');
  };

  const pickScanDir = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected && typeof selected === 'string') {
      setNewScanDir(selected);
    }
  };

  const removeScanDir = (dir: string) => {
    saveSettings({ scan_directories: settings.scan_directories.filter(d => d !== dir) });
  };

  return (
    <>
      {error && <div className="error-banner" style={{ margin: 0, marginBottom: 16 }}>{error}</div>}

      {/* Scan directories — zcode-style setting rows */}
      <div className="settings-section">
        <h3 className="settings-section-title">项目扫描目录</h3>
        <p className="settings-section-desc">Dev Workbench 会在以下目录中扫描 Git 仓库并显示在项目列表中。</p>

        {/* Existing directories */}
        {settings.scan_directories.map(dir => (
          <div key={dir} className="settings-row">
            <div className="settings-row-info">
              <span className="settings-row-label settings-row-label-mono">{dir}</span>
            </div>
            <div className="settings-row-control">
              <Button variant="ghost" size="sm" onClick={() => removeScanDir(dir)} title="移除">
                <IconX size={14} />
              </Button>
            </div>
          </div>
        ))}

        {settings.scan_directories.length === 0 && (
          <div className="settings-row">
            <div className="settings-row-info">
              <span className="settings-row-label" style={{ color: 'var(--text-tertiary)' }}>暂未添加扫描目录</span>
            </div>
          </div>
        )}

        {/* Add directory input */}
        <div className="settings-row" style={{ marginTop: 8 }}>
          <div className="settings-row-info" style={{ flex: 1 }}>
            <input
              className="settings-row-input"
              value={newScanDir}
              onChange={e => setNewScanDir(e.target.value)}
              placeholder="输入目录路径或点击右侧选择..."
              onKeyDown={e => { if (e.key === 'Enter') addScanDir(); }}
            />
          </div>
          <div className="settings-row-control">
            <Button variant="secondary" onClick={pickScanDir}>
              <IconFolderOpen size={14} />
              选择
            </Button>
            <Button variant="primary" onClick={addScanDir} disabled={!newScanDir.trim()}>
              添加
            </Button>
          </div>
        </div>
      </div>
    </>
  );
}
