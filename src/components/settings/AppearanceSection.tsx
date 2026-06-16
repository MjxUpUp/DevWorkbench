import { useState, useEffect } from 'react';
import { useSettingsStore } from '../../stores/settingsStore';
import type { ThemeMode } from '../../stores/settingsStore';
import { applyTheme, resolvedTheme, systemIsDark } from '../../utils/theme';

const MODES: { id: ThemeMode; label: string }[] = [
  { id: 'light', label: '浅色' },
  { id: 'dark', label: '深色' },
  { id: 'auto', label: '自动' },
];

export function AppearanceSection() {
  const settings = useSettingsStore((s) => s.settings);
  const saveSettings = useSettingsStore((s) => s.saveSettings);
  const error = useSettingsStore((s) => s.error);

  // The persisted mode (light/dark/auto) is the source of truth the user picks.
  const [mode, setMode] = useState<ThemeMode>(settings.theme);
  // What's actually rendered right now — lets "自动" show "(当前: 浅色)".
  const [actual, setActual] = useState<'light' | 'dark'>(resolvedTheme());

  useEffect(() => {
    setMode(settings.theme);
    setActual(resolvedTheme());
  }, [settings.theme]);

  const choose = (next: ThemeMode) => {
    setMode(next);
    applyTheme(next);
    setActual(resolvedTheme());
    saveSettings({ theme: next });
  };

  return (
    <>
      {error && <div className="error-banner" style={{ margin: 0, marginBottom: 16 }}>{error}</div>}

      <div className="settings-section">
        <h3 className="settings-section-title">界面主题</h3>

        {/* Theme — three-state segmented control (light / dark / auto-follow-system) */}
        <div className="settings-row">
          <div className="settings-row-info">
            <span className="settings-row-label">界面主题</span>
            <span className="settings-row-desc">
              选择应用界面的主色调外观。
              {mode === 'auto' && (
                <> 自动跟随系统（当前系统为 {systemIsDark() ? '深色' : '浅色'}，应用已切换为{actual === 'dark' ? '深色' : '浅色'}）。</>
              )}
            </span>
          </div>
          <div className="settings-row-control">
            <div className="settings-segmented">
              {MODES.map((m) => (
                <button
                  key={m.id}
                  className={`settings-segmented-btn ${mode === m.id ? 'active' : ''}`}
                  onClick={() => choose(m.id)}
                >
                  {m.label}
                </button>
              ))}
            </div>
          </div>
        </div>
      </div>
    </>
  );
}
