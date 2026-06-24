import { useState, useEffect } from 'react';
import { useSettingsStore } from '../../stores/settingsStore';
import type { ThemeMode } from '../../stores/settingsStore';
import { applyTheme, applyPalette, resolvedTheme, systemIsDark, type Palette } from '../../utils/theme';

const MODES: { id: ThemeMode; label: string }[] = [
  { id: 'light', label: '浅色' },
  { id: 'dark', label: '深色' },
  { id: 'auto', label: '自动' },
];

const PALETTES: { id: Palette; label: string; desc: string; swatch: string }[] = [
  { id: 'pi', label: 'pi.dev 暖纸', desc: '潮汐蓝 · 衡线 · 四角取景框', swatch: '#4b607c' },
  { id: 'ink', label: '墨砚 Ink-stone', desc: '朱砂红 · 思源宋体 · 东方水墨', swatch: '#8b2820' },
  { id: 'moss', label: '苔藓 Moss', desc: '苔藓绿 · Spectral · 有机自然', swatch: '#4a6b3a' },
];

export function AppearanceSection() {
  const settings = useSettingsStore((s) => s.settings);
  const saveSettings = useSettingsStore((s) => s.saveSettings);
  const error = useSettingsStore((s) => s.error);

  const [mode, setMode] = useState<ThemeMode>(settings.theme);
  const [palette, setPalette] = useState<Palette>(settings.palette ?? 'pi');
  const [actual, setActual] = useState<'light' | 'dark'>(resolvedTheme());

  useEffect(() => {
    setMode(settings.theme);
    setPalette(settings.palette ?? 'pi');
    setActual(resolvedTheme());
  }, [settings.theme, settings.palette]);

  const chooseMode = (next: ThemeMode) => {
    setMode(next);
    applyTheme(next);
    setActual(resolvedTheme());
    saveSettings({ theme: next });
  };

  const choosePalette = (next: Palette) => {
    setPalette(next);
    applyPalette(next);
    saveSettings({ palette: next });
  };

  return (
    <>
      {error && <div className="error-banner" style={{ margin: 0, marginBottom: 16 }}>{error}</div>}

      <div className="settings-section">
        <h3 className="settings-section-title">界面主题</h3>

        {/* Palette——三套风格主题（与亮/暗正交）*/}
        <div className="settings-row" style={{ flexDirection: 'column', alignItems: 'stretch' }}>
          <div className="settings-row-info">
            <span className="settings-row-label">风格主题</span>
            <span className="settings-row-desc">
              选择整体视觉风格（pi.dev 暖纸 / 墨砚 / 苔藓）。与亮/暗正交，共 6 种组合。
            </span>
          </div>
          <div className="settings-palette-grid" role="radiogroup" aria-label="风格主题">
            {PALETTES.map((p) => (
              <button
                key={p.id}
                type="button"
                role="radio"
                aria-checked={palette === p.id}
                className={`settings-palette-card ${palette === p.id ? 'active' : ''}`}
                onClick={() => choosePalette(p.id)}
              >
                <span className="settings-palette-swatch" style={{ background: p.swatch }} aria-hidden="true" />
                <span className="settings-palette-name">{p.label}</span>
                <span className="settings-palette-desc">{p.desc}</span>
              </button>
            ))}
          </div>
        </div>

        {/* Theme — three-state segmented control (light / dark / auto-follow-system) */}
        <div className="settings-row">
          <div className="settings-row-info">
            <span className="settings-row-label">亮/暗模式</span>
            <span className="settings-row-desc">
              选择应用界面的亮暗。
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
                  onClick={() => chooseMode(m.id)}
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
