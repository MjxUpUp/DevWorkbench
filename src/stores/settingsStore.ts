import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import { applyTheme, normalizeTheme } from '../utils/theme';

/** Theme is a three-state union so callers can't persist arbitrary strings. */
export type ThemeMode = 'light' | 'dark' | 'auto';

export interface AppSettings {
  scan_directories: string[];
  tool_paths: Record<string, string>;
  theme: ThemeMode;
  preferred_terminal: string;
  cli_flags: Record<string, string>;
}

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
  preferred_terminal: '',
  cli_flags: {},
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
      };
      set({ settings: merged, error: null });
      applyTheme(merged.theme);
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
    } catch (e) {
      console.error('Failed to save settings:', e);
      set({ error: String(e) });
    }
  },
}));
