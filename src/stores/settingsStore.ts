import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';

export interface AppSettings {
  scan_directories: string[];
  tool_paths: Record<string, string>;
  theme: string;
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
  theme: 'obsidian',
  preferred_terminal: '',
  cli_flags: {},
};

export const useSettingsStore = create<SettingsState>((set, get) => ({
  settings: DEFAULT_SETTINGS,
  error: null,

  loadSettings: async () => {
    try {
      const result = await invoke<AppSettings>('load_settings');
      set({ settings: { ...DEFAULT_SETTINGS, ...result }, error: null });
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
    } catch (e) {
      console.error('Failed to save settings:', e);
      set({ error: String(e) });
    }
  },
}));
