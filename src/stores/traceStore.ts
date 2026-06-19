import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { LlmTrace } from '../types';

/**
 * Read side of the LLM trace observability layer. The write side is
 * `trace::sink::DbTraceSink`, fire-and-forget from GlmChatModel; this store
 * pulls the persisted rows for ONE session so TraceView can render every LLM
 * HTTP call (req/resp body + status + latency) the turn made. Mirrors the
 * orchestrateStore invoke pattern.
 */
interface TraceState {
  /** Traces for the currently-selected session (oldest-first). null = not yet
   *  fetched; [] = fetched but the session made no LLM calls (failed before its
   *  first request, or a non-kernel agent with no sink). */
  traces: LlmTrace[] | null;
  loading: boolean;
  error: string | null;
  /** Load every LLM call trace for one session. Replaces any prior result so
   *  switching turns never shows the previous turn's traces. */
  fetchTraces: (sessionId: string) => Promise<void>;
  clear: () => void;
}

export const useTraceStore = create<TraceState>((set) => ({
  traces: null,
  loading: false,
  error: null,
  fetchTraces: async (sessionId) => {
    set({ loading: true, error: null });
    try {
      const traces = await invoke<LlmTrace[]>('list_llm_traces', { sessionId });
      set({ traces, loading: false });
    } catch (e) {
      set({ traces: null, loading: false, error: String(e) });
    }
  },
  clear: () => set({ traces: null, loading: false, error: null }),
}));
