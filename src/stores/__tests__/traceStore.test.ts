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
