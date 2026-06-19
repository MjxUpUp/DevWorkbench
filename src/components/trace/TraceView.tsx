import { useEffect, useState } from 'react';
import { useNavigationStore } from '../../stores/navigationStore';
import { useTraceStore } from '../../stores/traceStore';
import type { LlmTrace } from '../../types';

/**
 * LLM HTTP call timeline for one session (turn). Each row is one
 * GlmChatModel stream/generate call: # | model | status | latency | tokens |
 * error_kind. Click a row to expand the truncated request body and (on error)
 * the provider's real response body — the actual reason a turn failed.
 *
 * This is the observability payoff: a 0.8s "GLM stream failed: 400" turn is
 * now diagnosable end-to-end without guessing. Reuses the agent-block card +
 * collapse idiom from AgentMessage and the inline-timeline layout from
 * OrchestrateView. All color tokens fall back so a missing theme var never
 * blanks the row.
 */

type Badge = { label: string; color: string };

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
              const isOpen = expanded === t.id;
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
                    <span style={{ color: 'var(--text-tertiary)', minWidth: 96 }}>
                      {t.input_tokens != null || t.output_tokens != null
                        ? `${t.input_tokens ?? 0}/${t.output_tokens ?? 0} tok`
                        : '—'}
                    </span>
                    {t.error_kind && (
                      <span style={{ color: 'var(--danger, #dc2626)', fontSize: 'var(--text-xs)' }}>{t.error_kind}</span>
                    )}
                    <span style={{ marginLeft: 'auto', color: 'var(--text-tertiary)' }}>{isOpen ? '▾' : '▸'}</span>
                  </div>
                  {isOpen && (
                    <div style={{ padding: '8px 12px 12px', background: 'var(--bg-secondary, rgba(128,128,128,0.06))' }}>
                      <DetailSection title="Request body" body={t.req_body} />
                      {t.resp_body ? (
                        <DetailSection title="Response body (error)" body={t.resp_body} isError />
                      ) : (
                        t.error_kind && (
                          <div style={{ marginTop: 8, color: 'var(--text-tertiary)', fontSize: 'var(--text-xs)' }}>
                            无 response body（{t.error_kind}：调用未到达 HTTP，或 2xx 成功路径不落盘响应体）。
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
