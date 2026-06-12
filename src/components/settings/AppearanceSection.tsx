import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { AppSettings } from '../../types';

export function AppearanceSection() {
  const [settings, setSettings] = useState<AppSettings>({
    scan_directories: [],
    tool_paths: {},
    theme: 'light',
    preferred_terminal: '',
    cli_flags: {},
  });
  const [currentTheme, setCurrentTheme] = useState<'light' | 'dark'>('light');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<AppSettings>('load_settings')
      .then(data => { setSettings(data); setError(null); })
      .catch(e => setError(`加载设置失败: ${e}`));
  }, []);

  useEffect(() => {
    const theme = document.documentElement.getAttribute('data-theme');
    setCurrentTheme(theme === 'dark' ? 'dark' : 'light');
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

  return (
    <>
      {error && <div className="error-banner" style={{ margin: 0, marginBottom: 16 }}>{error}</div>}
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
    </>
  );
}
