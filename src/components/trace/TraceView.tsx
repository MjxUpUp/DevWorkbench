import { useEffect, useState } from 'react';
import { useNavigationStore } from '../../stores/navigationStore';
import { useTraceStore } from '../../stores/traceStore';
import type { LlmTrace } from '../../types';

/**
 * LLM HTTP call timeline for one session (turn). Each row is one
 * GlmChatModel stream/generate call: # | model | status | latency | tokens |
 * error_kind. Click a row to expand the truncated request body and the
 * provider's real response body — for errors the diagnostic reason a turn
 * failed, for a clean 2xx the full assistant output (persisted symmetric with
 * errors per the 2026-06-19 trace observability research).
 *
 * This is the observability payoff: a 0.8s "GLM stream failed: 400" turn is
 * now diagnosable end-to-end without guessing. Reuses the agent-block card +
 * collapse idiom from AgentMessage and the inline-timeline layout from
 * OrchestrateView. All color tokens fall back so a missing theme var never
 * blanks the row.
 */

type Badge = { label: string; color: string };

/** B3 slow-turn detection — mirrors the Rust `TimingChecker` thresholds exactly
 *  (DEFAULT_SLOW_TURN_MS = 60_000; slow_ttfb = threshold / 2 = 30_000) so the
 *  badge a user sees in TraceView matches the warn log the backend emitted. A
 *  turn is "slow" when total latency > 60s (slow_turn, precedence) OR ttfb >
 *  30s (slow_ttfb — model slow to start, distinct from slow output). null
 *  timing (pre-v18 rows / pure network failure) never flags. */
const SLOW_TURN_MS = 60_000;
const SLOW_TTFB_MS = 30_000;
function timingBadge(t: LlmTrace): Badge | null {
  if (t.latency_ms != null && t.latency_ms > SLOW_TURN_MS) {
    return { label: 'slow turn', color: 'var(--warning, #d97706)' };
  }
  if (t.ttfb_ms != null && t.ttfb_ms > SLOW_TTFB_MS) {
    return { label: 'slow ttfb', color: 'var(--warning, #d97706)' };
  }
  return null;
}

function statusBadge(t: LlmTrace): Badge {
  // Never-reached-HTTP failures: grey, the call died before a response.
  const preHttp = ['network', 'circuit', 'decode'];
  if (t.status_code == null) {
    return { label: t.error_kind ?? 'unknown', color: 'var(--text-tertiary, #888)' };
  }
  if (t.status_code >= 200 && t.status_code < 300) {
    return { label: String(t.status_code), color: 'var(--success, #16a34a)' };
  }
  // non_2xx — red, the diagnostic case this view exists for.
  if (preHttp.includes(t.error_kind ?? '')) {
    return { label: `${t.status_code}`, color: 'var(--text-tertiary, #888)' };
  }
  return { label: String(t.status_code), color: 'var(--danger, #dc2626)' };
}

export function TraceView() {
  const traceSessionId = useNavigationStore((s) => s.traceSessionId);
  const setActiveView = useNavigationStore((s) => s.setActiveView);
  const traces = useTraceStore((s) => s.traces);
  const loading = useTraceStore((s) => s.loading);
  const error = useTraceStore((s) => s.error);
  const fetchTraces = useTraceStore((s) => s.fetchTraces);
  const [expanded, setExpanded] = useState<string | null>(null);

  useEffect(() => {
    if (traceSessionId) void fetchTraces(traceSessionId);
  }, [traceSessionId, fetchTraces]);

  if (!traceSessionId) {
    return (
      <div className="chat-view">
        <div className="chat-empty">
          <h2>LLM Trace</h2>
          <p style={{ fontSize: 'var(--text-sm)', color: 'var(--text-tertiary)' }}>
            从某个会话的「🔍 Trace」按钮进入，查看该 turn 的每一次 LLM HTTP 调用。
          </p>
        </div>
      </div>
    );
  }

  return (
    <div className="chat-view">
      <div className="agent-message">
        <div className="agent-message-header">
          <span className="agent-block-title">LLM Trace</span>
          <span style={{ color: 'var(--text-tertiary)', fontSize: 'var(--text-xs)' }}>
            session: {traceSessionId.slice(0, 8)}
          </span>
          <button type="button" className="agent-message-copy-id" onClick={() => setActiveView('task')}>
            ← 返回对话
          </button>
        </div>

        {loading && (
          <div className="agent-block-body">
            <span style={{ color: 'var(--text-tertiary)' }}>加载中…</span>
          </div>
        )}
        {error && (
          <div className="agent-block-body">
            <span style={{ color: 'var(--danger, #dc2626)' }}>加载失败: {error}</span>
          </div>
        )}
        {!loading && !error && traces && traces.length === 0 && (
          <div className="agent-block-body">
            <span style={{ color: 'var(--text-tertiary)' }}>
              该会话没有 LLM 调用记录。可能在首次请求前就失败，或为非内核 agent（CLI 路径不接 trace sink）。
            </span>
          </div>
        )}

        {!loading && !error && traces && traces.length > 0 && (
          <div className="agent-block-body" style={{ padding: 0 }}>
            {traces.map((t, i) => {
              const badge = statusBadge(t);
              const slow = timingBadge(t);
              const isOpen = expanded === t.id;
              // 2xx success rows now also persist resp_body (the full assistant
              // output); only those render the body section as a normal (non-error)
              // response. Anything else carrying a body is an error diagnostic.
              const is2xx = t.status_code != null && t.status_code >= 200 && t.status_code < 300;
              return (
                <div key={t.id} style={{ borderBottom: '1px solid var(--border, rgba(128,128,128,0.2))' }}>
                  <div
                    onClick={() => setExpanded(isOpen ? null : t.id)}
                    style={{ display: 'flex', alignItems: 'center', gap: 12, padding: '8px 12px', cursor: 'pointer' }}
                  >
                    <span style={{ color: 'var(--text-tertiary)', minWidth: 28 }}>#{i + 1}</span>
                    <span style={{ minWidth: 140, fontWeight: 500 }}>{t.model}</span>
                    <span style={{ color: badge.color, fontWeight: 600, minWidth: 48 }}>{badge.label}</span>
                    <span style={{ color: 'var(--text-tertiary)', minWidth: 72 }}>
                      {t.latency_ms != null ? `${t.latency_ms}ms` : '—'}
                    </span>
                    {/* B3 ttfb/stream split — time-to-first-byte vs output time.
                        Shown inline only when present (post-v18 rows); the full
                        breakdown + "other" (send overhead) is in the expand. */}
                    <span style={{ color: 'var(--text-tertiary)', minWidth: 108, fontSize: 'var(--text-xs)' }}>
                      {t.ttfb_ms != null || t.stream_ms != null
                        ? `ttfb ${t.ttfb_ms ?? '—'} / stream ${t.stream_ms ?? '—'}`
                        : ''}
                    </span>
                    <span style={{ color: 'var(--text-tertiary)', minWidth: 96 }}>
                      {t.input_tokens != null || t.output_tokens != null
                        ? `${t.input_tokens ?? 0}/${t.output_tokens ?? 0} tok`
                        : '—'}
                    </span>
                    {slow && (
                      <span style={{ color: slow.color, fontSize: 'var(--text-xs)', fontWeight: 600 }}>{slow.label}</span>
                    )}
                    {t.error_kind && (
                      <span style={{ color: 'var(--danger, #dc2626)', fontSize: 'var(--text-xs)' }}>{t.error_kind}</span>
                    )}
                    <span style={{ marginLeft: 'auto', color: 'var(--text-tertiary)' }}>{isOpen ? '▾' : '▸'}</span>
                  </div>
                  {isOpen && (
                    <div style={{ padding: '8px 12px 12px', background: 'var(--bg-secondary, rgba(128,128,128,0.06))' }}>
                      <TimingBreakdown trace={t} />
                      <DetailSection title="Request body" body={t.req_body} />
                      {t.resp_body ? (
                        <DetailSection
                          title={is2xx ? 'Response body' : 'Response body (error)'}
                          body={t.resp_body}
                          isError={!is2xx}
                        />
                      ) : (
                        t.error_kind && (
                          <div style={{ marginTop: 8, color: 'var(--text-tertiary)', fontSize: 'var(--text-xs)' }}>
                            无 response body（{t.error_kind}：调用未到达 HTTP，没有响应体可记录）。
                          </div>
                        )
                      )}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        )}
      </div>
    </div>
  );
}

/** B3 per-call timing breakdown. latency = ttfb (send → first byte) + stream
 *  (first byte → completion) + "other" (send/parse overhead — the remainder when
 *  ttfb + stream don't sum to total latency). Renders nothing when no timing was
 *  captured (pre-v18 rows / pure network failure) so the expand stays clean for
 *  legacy data. */
function TimingBreakdown({ trace }: { trace: LlmTrace }) {
  if (trace.latency_ms == null && trace.ttfb_ms == null && trace.stream_ms == null) {
    return null;
  }
  const slow = timingBadge(trace);
  const total = trace.latency_ms ?? 0;
  const ttfb = trace.ttfb_ms ?? 0;
  const stream = trace.stream_ms ?? 0;
  const other = Math.max(0, total - ttfb - stream);
  // Bar widths: only meaningful when ttfb+stream ≤ total (legacy rows where
  // latency was captured but ttfb/stream weren't → a single grey total bar).
  const haveSplit = trace.ttfb_ms != null || trace.stream_ms != null;
  const denom = haveSplit && ttfb + stream > 0 ? ttfb + stream + other : 1;
  const fmt = (ms: number | null) => (ms != null ? `${ms}ms` : '—');
  return (
    <div style={{ marginTop: 8, marginBottom: 4 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 4 }}>
        <span style={{ fontWeight: 600 }}>Timing</span>
        <span style={{ color: 'var(--text-tertiary)', fontSize: 'var(--text-xs)' }}>
          total {fmt(trace.latency_ms)} = ttfb {fmt(trace.ttfb_ms)} + stream {fmt(trace.stream_ms)}
          {haveSplit && other > 0 ? ` + other ${other}ms` : ''}
        </span>
        {slow && (
          <span style={{ color: slow.color, fontSize: 'var(--text-xs)', fontWeight: 600 }}>· {slow.label}</span>
        )}
      </div>
      {haveSplit && (
        <div style={{ display: 'flex', height: 6, borderRadius: 3, overflow: 'hidden', background: 'var(--bg-tertiary, rgba(0,0,0,0.06))' }}>
          <div style={{ width: `${(ttfb / denom) * 100}%`, background: 'var(--accent, #2563eb)' }} title={`ttfb ${ttfb}ms`} />
          <div style={{ width: `${(stream / denom) * 100}%`, background: 'var(--success, #16a34a)' }} title={`stream ${stream}ms`} />
          {other > 0 && (
            <div style={{ width: `${(other / denom) * 100}%`, background: 'var(--text-tertiary, #888)' }} title={`other ${other}ms`} />
          )}
        </div>
      )}
    </div>
  );
}

function DetailSection({ title, body, isError }: { title: string; body: string; isError?: boolean }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(body);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    } catch {
      /* clipboard unavailable in non-secure context — non-fatal */
    }
  };
  // Pretty-print JSON when possible; show raw text otherwise (truncated bodies
  // may not be valid JSON if they hit the size cap with a ...(N more) suffix).
  let pretty = body;
  try {
    pretty = JSON.stringify(JSON.parse(body), null, 2);
  } catch {
    /* not JSON; render raw */
  }
  return (
    <div style={{ marginTop: 8 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginBottom: 4 }}>
        <span style={{ fontWeight: 600, color: isError ? 'var(--danger, #dc2626)' : 'inherit' }}>{title}</span>
        <button type="button" className="agent-message-copy-id" onClick={copy}>
          {copied ? '已复制' : '复制'}
        </button>
      </div>
      <pre
        style={{
          margin: 0,
          maxHeight: 360,
          overflow: 'auto',
          padding: 8,
          background: 'var(--bg-tertiary, rgba(0,0,0,0.06))',
          borderRadius: 4,
          fontSize: 'var(--text-xs)',
          whiteSpace: 'pre-wrap',
          wordBreak: 'break-word',
        }}
      >
        {pretty}
      </pre>
    </div>
  );
}
