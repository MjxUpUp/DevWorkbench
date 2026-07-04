import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { TraceSettings } from '../types';

/**
 * Trace retention settings store — the config side of the LLM trace
 * observability layer (the read side is `traceStore`). Powers the Settings →
 * Trace section: load the retention window, change it (null = infinite, per the
 * 2026-06-19 trace observability research), and trigger a manual prune + VACUUM.
 * The defaults live in the DB (`trace_settings` row); this store just loads and
 * mutates them via commands. Mirrors the traceStore invoke pattern.
 */
interface TraceSettingsState {
  settings: TraceSettings | null;
  loading: boolean;
  error: string | null;
  /** Rows removed by the most recent prune — surfaced in the UI ("已清理 N 条"). */
  lastPruned: number | null;
  fetchSettings: () => Promise<void>;
  /** Set retention (null = infinite) and prune immediately. Returns rows pruned. */
  setRetention: (days: number | null) => Promise<number>;
  /** Prune now + VACUUM, regardless of whether retention changed. */
  pruneNow: () => Promise<number>;
}

export const useTraceSettingsStore = create<TraceSettingsState>((set, get) => ({
  settings: null,
  loading: false,
  error: null,
  lastPruned: null,
  fetchSettings: async () => {
    set({ loading: true, error: null });
    try {
      const settings = await invoke<TraceSettings>('get_trace_settings_cmd');
      set({ settings, loading: false });
    } catch (e) {
      set({ settings: null, loading: false, error: String(e) });
    }
  },
  setRetention: async (days) => {
    set({ error: null });
    try {
      const pruned = await invoke<number>('set_trace_retention_cmd', { days });
      await get().fetchSettings();
      set({ lastPruned: pruned });
      return pruned;
    } catch (e) {
      set({ error: String(e) });
      return 0;
    }
  },
  pruneNow: async () => {
    set({ error: null });
    try {
      const pruned = await invoke<number>('prune_llm_traces_now');
      await get().fetchSettings();
      set({ lastPruned: pruned });
      return pruned;
    } catch (e) {
      set({ error: String(e) });
      return 0;
    }
  },
}));
