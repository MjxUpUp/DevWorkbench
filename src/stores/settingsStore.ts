import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { applyTheme, normalizeTheme, applyPalette, normalizePalette } from '../utils/theme';
import type { AppSettings } from '../types';

/** Theme is a three-state union so callers can't persist arbitrary strings. */
export type ThemeMode = 'light' | 'dark' | 'auto';
/** Palette 是三套风格主题（与亮/暗正交）。 */
export type PaletteMode = 'pi' | 'ink' | 'moss';

interface SettingsState {
  settings: AppSettings;
  error: string | null;
  loadSettings: () => Promise<void>;
  saveSettings: (patch: Partial<AppSettings>) => Promise<void>;
}

const DEFAULT_SETTINGS: AppSettings = {
  scan_directories: [],
  tool_paths: {},
  theme: 'auto',
  palette: 'pi',
  cli_flags: {},
  onboarding_completed: false,
};

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: DEFAULT_SETTINGS,
  error: null,

  loadSettings: async () => {
    try {
      const result = await invoke<AppSettings>('load_settings');
      const merged = {
        ...DEFAULT_SETTINGS,
        ...result,
        // Coerce whatever was persisted (e.g. legacy 'obsidian') into a safe value.
        theme: normalizeTheme(result?.theme),
        palette: normalizePalette(result?.palette),
      };
      set({ settings: merged, error: null });
      applyTheme(merged.theme);
      applyPalette(merged.palette);
    } catch (e) {
      console.error('Failed to load settings:', e);
      set({ error: String(e) });
    }
  },

  saveSettings: async (patch) => {
    try {
      const next = { ...get().settings, ...patch };
      await invoke('save_settings', { settings: next });
      set({ settings: next, error: null });
      if (patch.theme) applyTheme(patch.theme);
      if (patch.palette) applyPalette(patch.palette);
    } catch (e) {
      console.error('Failed to save settings:', e);
      set({ error: String(e) });
    }
  },
}));
