import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { LlmTrace } from '../types';
import type { VerdictRow } from '../utils/evalApi';

/**
 * Read side of the trace observability layer. The write side is
 * `trace::sink::DbTraceSink`, fire-and-forget from the ChatModel; this store
 * pulls the persisted rows for ONE session so TraceView can render every LLM
 * HTTP call (req/resp body + status + latency) the turn made.
 *
 * verdicts 旁路：human-gate 审批决策（verdicts 表 gate='human-gate'）——blocks 流里
 * approval_required 被 react_chat 过滤（react_chat.rs），所以审批节点只能从 verdicts
 * join 进 TraceView。
 */
interface TraceState {
  /** Traces for the currently-selected session (oldest-first). null = not yet
   *  fetched; [] = fetched but the session made no LLM calls (failed before its
   *  first request, or a non-kernel agent with no sink). */
  traces: LlmTrace[] | null;
  /** human-gate verdicts for the selected session (the approval ledger). [] on
   *  a session with no destructive ops or before the fetch resolves. */
  verdicts: VerdictRow[];
  loading: boolean;
  error: string | null;
  /** Load every LLM call trace for one session. Replaces any prior result so
   *  switching turns never shows the previous turn's traces. */
  fetchTraces: (sessionId: string) => Promise<void>;
  /** Load the session's human-gate verdicts (approval ledger). */
  fetchVerdicts: (sessionId: string) => Promise<void>;
  clear: () => void;
}

export const useTraceStore = create<TraceState>((set) => {
  // Two independent monotonic ids — one per fetch kind — so a slow in-flight
  // fetch can't clobber a faster, newer one. Switching sessions fires fetchTraces
  // + fetchVerdicts in parallel; without guards the slower (older) response of
  // either wins if it resolves last, showing the wrong turn's traces/verdicts.
  // Separate ids (not shared) because the two fetches race independently — a
  // shared id would make each invalidate the other on every switch.
  let traceSeq = 0;
  let verdictSeq = 0;
  return {
    traces: null,
    verdicts: [],
    loading: false,
    error: null,
    fetchTraces: async (sessionId) => {
      const myId = ++traceSeq;
      set({ loading: true, error: null });
      try {
        const traces = await invoke<LlmTrace[]>('list_llm_traces', { sessionId });
        if (myId !== traceSeq) return; // superseded by a newer fetch — drop stale result
        set({ traces, loading: false });
      } catch (e) {
        if (myId !== traceSeq) return;
        set({ traces: null, loading: false, error: String(e) });
      }
    },
    fetchVerdicts: async (sessionId) => {
      const myId = ++verdictSeq;
      try {
        const rows = await invoke<VerdictRow[]>('list_verdicts', { sessionId, gate: 'human-gate' });
        if (myId !== verdictSeq) return; // superseded — drop stale verdicts from another session
        // invoke may resolve null when the command is unmocked (e2e harness) or
        // returns no rows — coerce to [] so the component's `.filter` never sees null.
        set({ verdicts: rows ?? [] });
      } catch {
        if (myId !== verdictSeq) return;
        // Verdicts are supplemental (审批节点); a failure shouldn't blank the
        // whole view — the LLM trace + blocks still render.
        set({ verdicts: [] });
      }
    },
    clear: () => {
      traceSeq += 1; // invalidate any in-flight fetch
      verdictSeq += 1;
      set({ traces: null, verdicts: [], loading: false, error: null });
    },
  };
});
