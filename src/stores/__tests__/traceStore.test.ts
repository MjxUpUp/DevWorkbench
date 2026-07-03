import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { useTraceStore } from '../traceStore';
import type { LlmTrace } from '../../types';

const trace400: LlmTrace = {
  id: 't1',
  session_id: 'sess-1',
  conversation_id: 'conv-1',
  model: 'glm-4.6',
  base_url: 'https://api.example',
  status_code: 400,
  error_kind: 'non_2xx',
  req_body: '{"model":"glm-4.6"}',
  resp_body: 'invalid_request_error',
  latency_ms: 8,
  input_tokens: null,
  output_tokens: null,
  ttfb_ms: null,
  stream_ms: null,
  span_id: null,
  parent_span_id: null,
  span_name: null,
  created_at: '2026-06-19T00:00:00Z',
};

describe('traceStore.fetchTraces', () => {
  beforeEach(() => {
    useTraceStore.setState({ traces: null, loading: false, error: null });
    vi.clearAllMocks();
  });

  it('loads traces via list_llm_traces with the camelCase sessionId arg', async () => {
    // Tauri maps the JS camelCase arg to the Rust snake_case `session_id` param;
    // asserting the wire shape here guards that mapping (a wrong key silently
    // returns every- or no- traces).
    vi.mocked(invoke).mockResolvedValue([trace400]);
    await useTraceStore.getState().fetchTraces('sess-1');
    expect(invoke).toHaveBeenCalledWith('list_llm_traces', { sessionId: 'sess-1' });
    expect(useTraceStore.getState().traces).toEqual([trace400]);
    expect(useTraceStore.getState().loading).toBe(false);
    expect(useTraceStore.getState().error).toBeNull();
  });

  it('stores [] (not an error) when the session made no LLM calls', async () => {
    // Empty vs error is a real UI distinction: [] = "nothing to show" (failed
    // before first request, or non-kernel agent), error = "trace store broken".
    vi.mocked(invoke).mockResolvedValue([]);
    await useTraceStore.getState().fetchTraces('sess-empty');
    expect(useTraceStore.getState().traces).toEqual([]);
    expect(useTraceStore.getState().error).toBeNull();
  });

  it('captures the error string and clears traces when invoke rejects', async () => {
    vi.mocked(invoke).mockRejectedValue(new Error('db lock'));
    await useTraceStore.getState().fetchTraces('sess-1');
    expect(useTraceStore.getState().traces).toBeNull();
    expect(useTraceStore.getState().error).toContain('db lock');
    expect(useTraceStore.getState().loading).toBe(false);
  });

  it('clear() resets traces / loading / error', () => {
    useTraceStore.setState({ traces: [trace400], loading: true, error: 'x' });
    useTraceStore.getState().clear();
    expect(useTraceStore.getState().traces).toBeNull();
    expect(useTraceStore.getState().loading).toBe(false);
    expect(useTraceStore.getState().error).toBeNull();
  });
});

describe('traceStore — fetch race guard', () => {
  beforeEach(() => {
    useTraceStore.setState({ traces: null, loading: false, error: null });
    vi.clearAllMocks();
  });

  // A promise whose resolution we control, so a test can resolve fetches in a
  // chosen order and assert the race guard keeps the right one.
  function deferred<T>() {
    let resolve!: (v: T) => void;
    const promise = new Promise<T>((r) => {
      resolve = r;
    });
    return { promise, resolve };
  }

  it('drops the result of a superseded (slower, older) fetch', async () => {
    const slowA = deferred<LlmTrace[]>();
    const fastB = deferred<LlmTrace[]>();
    vi.mocked(invoke).mockReturnValueOnce(slowA.promise).mockReturnValueOnce(fastB.promise);

    const pA = useTraceStore.getState().fetchTraces('A'); // started first → stale
    const pB = useTraceStore.getState().fetchTraces('B'); // started second → supersedes A

    // B resolves first (fast) — its result applies.
    fastB.resolve([trace400]);
    await pB;
    expect(useTraceStore.getState().traces).toEqual([trace400]);

    // A resolves last (slow) — stale, must NOT clobber B. Without the guard the
    // older fetch would win and the UI would show the wrong turn's traces.
    slowA.resolve([{ ...trace400, id: 'stale' }]);
    await pA;
    expect(useTraceStore.getState().traces).toEqual([trace400]);
    expect(useTraceStore.getState().loading).toBe(false);
  });

  it('clear() invalidates an in-flight fetch so its late result never lands', async () => {
    const pending = deferred<LlmTrace[]>();
    vi.mocked(invoke).mockReturnValueOnce(pending.promise);
    const p = useTraceStore.getState().fetchTraces('A');
    useTraceStore.getState().clear(); // bump seq → pending result is now stale
    pending.resolve([{ ...trace400, id: 'late' }]);
    await p;
    const s = useTraceStore.getState();
    expect(s.traces).toBeNull();
    expect(s.loading).toBe(false);
    expect(s.error).toBeNull();
  });
});
