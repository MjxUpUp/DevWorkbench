import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { TraceView } from '../TraceView';
import { useNavigationStore } from '../../../stores/navigationStore';
import { useTraceStore } from '../../../stores/traceStore';
import type { LlmTrace } from '../../../types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

const trace400: LlmTrace = {
  id: 't1',
  session_id: 'sess-1',
  conversation_id: null,
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

const trace200: LlmTrace = {
  ...trace400,
  id: 't2',
  status_code: 200,
  error_kind: null,
  resp_body: null,
  latency_ms: 120,
  input_tokens: 10,
  output_tokens: 5,
};

describe('TraceView', () => {
  beforeEach(() => {
    useNavigationStore.setState({ traceSessionId: 'sess-1' });
    useTraceStore.setState({ traces: null, loading: false, error: null });
    vi.clearAllMocks();
  });

  it('renders the empty-state hint when no session is selected', () => {
    useNavigationStore.setState({ traceSessionId: null });
    render(<TraceView />);
    expect(screen.getByText('会话 Trace')).toBeInTheDocument();
    // The hint points the user to the per-turn entry button.
    expect(screen.getByText(/🔍 Trace/)).toBeInTheDocument();
  });

  it('renders one row per trace with status badge, latency, tokens', async () => {
    vi.mocked(invoke).mockResolvedValue([trace400, trace200]);
    render(<TraceView />);
    await waitFor(() => {
      expect(useTraceStore.getState().traces).toHaveLength(2);
    });
    // model + both status badges + latency + tokens present
    expect(screen.getAllByText('glm-4.6').length).toBeGreaterThan(0);
    expect(screen.getByText('400')).toBeInTheDocument();
    expect(screen.getByText('200')).toBeInTheDocument();
    expect(screen.getByText('8ms')).toBeInTheDocument();
    // 10/5 tok shows in BOTH the 概要 summary and the row (summary is new in the
    // Langfuse-style view) — assert presence, not uniqueness.
    expect(screen.getAllByText('10/5 tok').length).toBeGreaterThanOrEqual(1);
  });

  it('expands a row to reveal the error response body (the diagnostic payload)', async () => {
    // This is the whole point of the feature: a failed turn's real 400 body must
    // be one click away. Hidden until the row expands.
    vi.mocked(invoke).mockResolvedValue([trace400]);
    render(<TraceView />);
    await waitFor(() => expect(useTraceStore.getState().traces).toHaveLength(1));
    expect(screen.queryByText(/invalid_request_error/)).not.toBeInTheDocument();
    // Click the 400 badge — its parent row's onClick toggles expansion.
    fireEvent.click(screen.getByText('400'));
    expect(await screen.findByText(/invalid_request_error/)).toBeInTheDocument();
  });

  it('explains an empty trace set instead of looking broken', async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    render(<TraceView />);
    await waitFor(() => expect(useTraceStore.getState().traces).toEqual([]));
    expect(screen.getByText(/没有可追溯的链路数据/)).toBeInTheDocument();
  });

  it('surfaces a load failure as an error message', async () => {
    vi.mocked(invoke).mockRejectedValue(new Error('db lock'));
    render(<TraceView />);
    await waitFor(() => expect(useTraceStore.getState().error).toContain('db lock'));
    expect(screen.getByText(/加载失败/)).toBeInTheDocument();
  });

  it('shows the ttfb/stream split inline and a slow-turn badge over 60s', async () => {
    // B3: latency > 60_000 must flag "slow turn" (mirrors the Rust TimingChecker
    // DEFAULT_SLOW_TURN_MS), and the ttfb/stream split must render inline so a
    // user sees time-to-first-byte vs output time without expanding.
    const slow: LlmTrace = {
      ...trace400,
      id: 'slow1',
      latency_ms: 75_000,
      ttfb_ms: 12_000,
      stream_ms: 60_000,
    };
    vi.mocked(invoke).mockResolvedValue([slow]);
    render(<TraceView />);
    await waitFor(() => expect(useTraceStore.getState().traces).toHaveLength(1));
    expect(screen.getByText('ttfb 12000 / stream 60000')).toBeInTheDocument();
    expect(screen.getByText('slow turn')).toBeInTheDocument();
  });

  it('flags slow ttfb (>30s) even when total latency is under 60s', async () => {
    // A 45s turn that spent 35s before the first byte is "slow to start", not
    // "slow to output" — the distinct diagnosis ttfb_ms exists to surface.
    const slowStart: LlmTrace = {
      ...trace400,
      id: 'slow2',
      latency_ms: 45_000,
      ttfb_ms: 35_000,
      stream_ms: 8_000,
    };
    vi.mocked(invoke).mockResolvedValue([slowStart]);
    render(<TraceView />);
    await waitFor(() => expect(useTraceStore.getState().traces).toHaveLength(1));
    expect(screen.getByText('slow ttfb')).toBeInTheDocument();
    expect(screen.queryByText('slow turn')).not.toBeInTheDocument();
  });

  it('renders the timing breakdown equation in the expanded row', async () => {
    const timed: LlmTrace = {
      ...trace400,
      id: 'timed1',
      latency_ms: 5_000,
      ttfb_ms: 1_200,
      stream_ms: 3_500,
    };
    vi.mocked(invoke).mockResolvedValue([timed]);
    render(<TraceView />);
    await waitFor(() => expect(useTraceStore.getState().traces).toHaveLength(1));
    fireEvent.click(screen.getByText('400'));
    // total = ttfb + stream (+ other for the remainder) — the breakdown equation.
    expect(await screen.findByText(/total 5000ms = ttfb 1200ms \+ stream 3500ms/)).toBeInTheDocument();
  });

  it('groups traces into a span tree (root agent + nested subagent)', async () => {
    // A1: when traces carry span context, TraceView stops rendering a flat
    // timeline and groups calls by the agent that issued them — a root "agent"
    // span with a "subagent" child nested under it, mirroring the agent-DAG
    // the backend attributed via fork_with_counting_cost → SpanContext.
    const rootCall: LlmTrace = {
      ...trace400,
      id: 'root-call',
      status_code: 200,
      error_kind: null,
      latency_ms: 100,
      input_tokens: 50,
      output_tokens: 10,
      span_id: 'span-root',
      parent_span_id: null,
      span_name: 'agent',
      created_at: '2026-07-03T00:00:00Z',
    };
    const childCall: LlmTrace = {
      ...trace400,
      id: 'child-call',
      // 400 — surfaces as a failure badge on the subagent span header.
      span_id: 'span-child',
      parent_span_id: 'span-root',
      span_name: 'subagent',
      created_at: '2026-07-03T00:00:01Z',
    };
    vi.mocked(invoke).mockResolvedValue([rootCall, childCall]);
    render(<TraceView />);
    await waitFor(() => expect(useTraceStore.getState().traces).toHaveLength(2));
    // Both span group headers render — the tree, not a flat list.
    expect(screen.getByText('agent')).toBeInTheDocument();
    expect(screen.getByText('subagent')).toBeInTheDocument();
    // The subagent's one failed call surfaces on its header (1 失败).
    expect(screen.getByText('1 失败')).toBeInTheDocument();
    // Both calls' rows are present (model + status badges).
    expect(screen.getAllByText('glm-4.6').length).toBeGreaterThanOrEqual(2);
    expect(screen.getByText('200')).toBeInTheDocument();
    expect(screen.getByText('400')).toBeInTheDocument();
  });

  it('renders a legacy (span-less) session as a flat timeline', async () => {
    // Backward compat: pre-A1 sessions (no span_id on any trace) must NOT show
    // span group headers — they render the original flat row list unchanged.
    vi.mocked(invoke).mockResolvedValue([
      { ...trace400, span_id: null, parent_span_id: null, span_name: null },
      { ...trace200, span_id: null, parent_span_id: null, span_name: null },
    ]);
    render(<TraceView />);
    await waitFor(() => expect(useTraceStore.getState().traces).toHaveLength(2));
    // No span group headers (agent / subagent / 次调用) in flat mode.
    expect(screen.queryByTestId('span-group-header')).not.toBeInTheDocument();
    // Flat rows still render both calls.
    expect(screen.getByText('400')).toBeInTheDocument();
    expect(screen.getByText('200')).toBeInTheDocument();
  });
});
