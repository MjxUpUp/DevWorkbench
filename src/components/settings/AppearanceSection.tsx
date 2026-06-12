import { useState, useEffect } from 'react';
import { useSettingsStore } from '../../stores/settingsStore';

export function AppearanceSection() {
  const settings = useSettingsStore((s) => s.settings);
  const saveSettings = useSettingsStore((s) => s.saveSettings);
  const error = useSettingsStore((s) => s.error);

  const [currentTheme, setCurrentTheme] = useState<'light' | 'dark'>('light');

  useEffect(() => {
    const theme = document.documentElement.getAttribute('data-theme');
    setCurrentTheme(theme === 'dark' ? 'dark' : 'light');
  }, []);

  const setTheme = (theme: 'light' | 'dark') => {
    setCurrentTheme(theme);
    if (theme === 'dark') {
      document.documentElement.setAttribute('data-theme', 'dark');
    } else {
      document.documentElement.removeAttribute('data-theme');
    }
    saveSettings({ theme });
  };

  return (
    <>
      {error && <div className="error-banner" style={{ margin: 0, marginBottom: 16 }}>{error}</div>}

      <div className="settings-section">
        <h3 className="settings-section-title">界面主题</h3>

        {/* Theme — zcode-style settings row with selector buttons */}
        <div className="settings-row">
          <div className="settings-row-info">
            <span className="settings-row-label">界面主题</span>
            <span className="settings-row-desc">选择应用界面的主色调外观。</span>
          </div>
          <div className="settings-row-control">
            <div className="settings-segmented">
              <button
                className={`settings-segmented-btn ${currentTheme === 'light' ? 'active' : ''}`}
                onClick={() => setTheme('light')}
              >
                浅色
              </button>
              <button
                className={`settings-segmented-btn ${currentTheme === 'dark' ? 'active' : ''}`}
                onClick={() => setTheme('dark')}
              >
                深色
              </button>
            </div>
          </div>
        </div>
      </div>
    </>
  );
}
