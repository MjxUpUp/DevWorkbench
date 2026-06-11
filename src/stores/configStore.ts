import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { McpConfigFile } from '../types';

interface ConfigState {
  mcpConfig: McpConfigFile | null;
  loading: boolean;

  loadConfig: (projectPath: string) => Promise<void>;
  saveConfig: (projectPath: string, config: McpConfigFile) => Promise<void>;
  applyConfig: (projectPath: string, config: McpConfigFile) => Promise<string[]>;
}

export const useConfigStore = create<ConfigState>((set) => ({
  mcpConfig: null,
  loading: false,

  loadConfig: async (projectPath) => {
    set({ loading: true });
    try {
      const config = await invoke<McpConfigFile>('load_mcp_config', { projectPath });
      set({ mcpConfig: config });
    } catch (e) {
      console.error('Load MCP config failed:', e);
    } finally {
      set({ loading: false });
    }
  },

  saveConfig: async (projectPath, config) => {
    await invoke('save_mcp_config', { projectPath, config });
    set({ mcpConfig: config });
  },

  applyConfig: async (projectPath, config) => {
    return invoke<string[]>('apply_mcp_config', { projectPath, config });
  },
}));
