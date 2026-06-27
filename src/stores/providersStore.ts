import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { ProvidersConfig, ProtocolKind } from '../types';

/** Result of probing one provider's credentials (mirrors the Rust struct). */
export interface ProviderTestResult {
  ok: boolean;
  status: number;
  message: string;
}

interface ProvidersState {
  config: ProvidersConfig | null;
  loading: boolean;
  loadProviders: () => Promise<void>;
  saveProviders: (config: ProvidersConfig) => Promise<void>;
  testProvider: (
    endpoint: string,
    apiKey: string,
    model: string,
    protocol: ProtocolKind,
  ) => Promise<ProviderTestResult>;
}

/**
 * Store for the GLOBAL providers.toml (lives in the app data dir, shared across
 * projects — unlike mcp-servers.toml which is per-project). The transparent
 * kernel agent (ReactAgent) resolves its endpoint+key from this config at run
 * time, matching the requested model against enabled providers.
 */
export const useProvidersStore = create<ProvidersState>((set) => ({
  config: null,
  loading: false,

  loadProviders: async () => {
    set({ loading: true });
    try {
      const config = await invoke<ProvidersConfig>('get_providers_config');
      set({ config });
    } catch (e) {
      console.error('Load providers config failed:', e);
    } finally {
      set({ loading: false });
    }
  },

  saveProviders: async (config) => {
    await invoke('set_providers_config', { config });
    set({ config });
  },

  testProvider: async (endpoint, apiKey, model, protocol) => {
    return invoke<ProviderTestResult>('test_provider_connection', {
      endpoint,
      apiKey,
      model,
      protocol,
    });
  },
}));
