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
 * collapse idiom from AgentMessage. All color tokens fall back so a missing theme var never
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
    return { label: 'slow turn', color: 'var(--warning)' };
  }
  if (t.ttfb_ms != null && t.ttfb_ms > SLOW_TTFB_MS) {
    return { label: 'slow ttfb', color: 'var(--warning)' };
  }
  return null;
}

function statusBadge(t: LlmTrace): Badge {
  // Never-reached-HTTP failures: grey, the call died before a response.
  const preHttp = ['network', 'circuit', 'decode'];
  if (t.status_code == null) {
    return { label: t.error_kind ?? 'unknown', color: 'var(--text-tertiary)' };
  }
  if (t.status_code >= 200 && t.status_code < 300) {
    return { label: String(t.status_code), color: 'var(--success)' };
  }
  // non_2xx — red, the diagnostic case this view exists for.
  if (preHttp.includes(t.error_kind ?? '')) {
    return { label: `${t.status_code}`, color: 'var(--text-tertiary)' };
  }
  return { label: String(t.status_code), color: 'var(--danger)' };
}

// ---- A1 span tree (OTel-aligned trace attribution) ----

/** A span node in the trace tree: one per agent instance. Holds the calls that
 *  agent made (ASC) + its child spans (the sub-agents it dispatched). Built from
 *  the flat trace list by grouping on span_id and linking via parent_span_id —
 *  the same parent/child relationship an OTel tracer emits, derived here from
 *  the per-call span context the ChatModel stamps at record time. */
type SpanNode = {
  spanId: string;
  name: string;
  parent: string | null;
  traces: LlmTrace[];
  children: SpanNode[];
};

/** Synthetic id for traces with no span_id (pre-v22 rows / ad-hoc agents). They
 *  bucket into one root-level "unattributed" group so a mixed session still
 *  renders every call. */
const UNATTRIBUTED = '__dw_unattributed__';

/** True when at least one trace carries a span_id — i.e. the session was
 *  recorded post-A1 by a span-attributed agent. When false, TraceView renders
 *  the legacy flat timeline (backward-compatible with all pre-A1 sessions). */
function hasSpans(traces: LlmTrace[]): boolean {
  return traces.some((t) => t.span_id != null);
}

function cmpCreated(a: LlmTrace, b: LlmTrace): number {
  return a.created_at < b.created_at ? -1 : a.created_at > b.created_at ? 1 : 0;
}

/** Group a flat trace list into a span forest. Each unique span_id becomes a
 *  node; parent_span_id links a node under its orchestrator's node. A node
 *  whose parent isn't in the set (the orchestrator made no LLM calls itself) is
 *  treated as a root. Root + child order is deterministic by first-call time. */
function buildSpanForest(traces: LlmTrace[]): SpanNode[] {
  const buckets = new Map<string, LlmTrace[]>();
  const parentOf = new Map<string, string | null>();
  const nameOf = new Map<string, string>();
  for (const t of traces) {
    const key = t.span_id ?? UNATTRIBUTED;
    const arr = buckets.get(key);
    if (arr) arr.push(t);
    else buckets.set(key, [t]);
    if (!parentOf.has(key)) {
      parentOf.set(key, t.span_id == null ? null : t.parent_span_id);
    }
    if (!nameOf.has(key)) {
      nameOf.set(key, t.span_id == null ? '无 span 归属' : t.span_name ?? 'span');
    }
  }
  const nodes = new Map<string, SpanNode>();
  for (const [key, arr] of buckets) {
    arr.sort(cmpCreated);
    nodes.set(key, {
      spanId: key,
      name: nameOf.get(key) ?? 'span',
      parent: parentOf.get(key) ?? null,
      traces: arr,
      children: [],
    });
  }
  const roots: SpanNode[] = [];
  for (const node of nodes.values()) {
    const parent = node.parent != null ? nodes.get(node.parent) : undefined;
    if (parent) parent.children.push(node);
    else roots.push(node);
  }
  const byFirstCall = (a: SpanNode, b: SpanNode) => cmpCreated(a.traces[0], b.traces[0]);
  roots.sort(byFirstCall);
  for (const node of nodes.values()) node.children.sort(byFirstCall);
  return roots;
}

/** One trace row (button + expand). Extracted so the flat timeline and the span
 *  tree render identical rows — the expand/detail UX doesn't diverge by mode. */
function TraceRow({
  trace,
  index,
  expanded,
  onToggle,
}: {
  trace: LlmTrace;
  index: number;
  expanded: boolean;
  onToggle: () => void;
}) {
  const badge = statusBadge(trace);
  const slow = timingBadge(trace);
  const is2xx =
    trace.status_code != null && trace.status_code >= 200 && trace.status_code < 300;
  return (
    <div style={{ borderBottom: '1px solid var(--border)' }}>
      <button
        type="button"
        aria-expanded={expanded}
        onClick={onToggle}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 12,
          width: '100%',
          padding: '8px 12px',
          background: 'transparent',
          border: 'none',
          textAlign: 'left',
          font: 'inherit',
          color: 'inherit',
          cursor: 'pointer',
        }}
      >
        <span style={{ color: 'var(--text-tertiary)', minWidth: 28 }}>#{index + 1}</span>
        <span style={{ minWidth: 140, fontWeight: 500 }}>{trace.model}</span>
        <span style={{ color: badge.color, fontWeight: 600, minWidth: 48 }}>{badge.label}</span>
        <span style={{ color: 'var(--text-tertiary)', minWidth: 72 }}>
          {trace.latency_ms != null ? `${trace.latency_ms}ms` : '—'}
        </span>
        <span style={{ color: 'var(--text-tertiary)', minWidth: 108, fontSize: 'var(--text-xs)' }}>
          {trace.ttfb_ms != null || trace.stream_ms != null
            ? `ttfb ${trace.ttfb_ms ?? '—'} / stream ${trace.stream_ms ?? '—'}`
            : ''}
        </span>
        <span style={{ color: 'var(--text-tertiary)', minWidth: 96 }}>
          {trace.input_tokens != null || trace.output_tokens != null
            ? `${trace.input_tokens ?? 0}/${trace.output_tokens ?? 0} tok`
            : '—'}
        </span>
        {slow && (
          <span style={{ color: slow.color, fontSize: 'var(--text-xs)', fontWeight: 600 }}>
            {slow.label}
          </span>
        )}
        {trace.error_kind && (
          <span style={{ color: 'var(--danger)', fontSize: 'var(--text-xs)' }}>{trace.error_kind}</span>
        )}
        <span style={{ marginLeft: 'auto', color: 'var(--text-tertiary)' }}>{expanded ? '▾' : '▸'}</span>
      </button>
      {expanded && (
        <div style={{ padding: '8px 12px 12px', background: 'var(--surface-2)' }}>
          <TimingBreakdown trace={trace} />
          <DetailSection title="Request body" body={trace.req_body} />
          {trace.resp_body ? (
            <DetailSection
              title={is2xx ? 'Response body' : 'Response body (error)'}
              body={trace.resp_body}
              isError={!is2xx}
            />
          ) : (
            trace.error_kind && (
              <div style={{ marginTop: 8, color: 'var(--text-tertiary)', fontSize: 'var(--text-xs)' }}>
                无 response body（{trace.error_kind}：调用未到达 HTTP，没有响应体可记录）。
              </div>
            )
          )}
        </div>
      )}
    </div>
  );
}

/** Recursive span group: header (name + call count + failures + tokens) then
 *  this span's trace rows, then child spans indented under a left rule. */
function SpanGroup({
  node,
  depth,
  indexMap,
  expanded,
  onToggle,
}: {
  node: SpanNode;
  depth: number;
  indexMap: Map<string, number>;
  expanded: string | null;
  onToggle: (id: string) => void;
}) {
  const failCount = node.traces.filter(
    (t) => t.status_code == null || t.status_code >= 400,
  ).length;
  const inTok = node.traces.reduce((s, t) => s + (t.input_tokens ?? 0), 0);
  const outTok = node.traces.reduce((s, t) => s + (t.output_tokens ?? 0), 0);
  return (
    <div
      style={{
        marginLeft: depth > 0 ? 16 : 0,
        borderLeft: depth > 0 ? '2px solid var(--border)' : 'none',
        paddingLeft: depth > 0 ? 8 : 0,
      }}
    >
      <div
        data-testid="span-group-header"
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 8,
          padding: '6px 12px',
          background: 'var(--surface-2)',
        }}
      >
        <span style={{ fontWeight: 600 }}>{node.name}</span>
        <span style={{ color: 'var(--text-tertiary)', fontSize: 'var(--text-xs)' }}>
          {node.traces.length} 次调用
        </span>
        {failCount > 0 && (
          <span style={{ color: 'var(--danger)', fontSize: 'var(--text-xs)' }}>{failCount} 失败</span>
        )}
        {(inTok > 0 || outTok > 0) && (
          <span style={{ color: 'var(--text-tertiary)', fontSize: 'var(--text-xs)' }}>
            {inTok}/{outTok} tok
          </span>
        )}
      </div>
      {node.traces.map((t) => (
        <TraceRow
          key={t.id}
          trace={t}
          index={indexMap.get(t.id) ?? 0}
          expanded={expanded === t.id}
          onToggle={() => onToggle(t.id)}
        />
      ))}
      {node.children.map((c) => (
        <SpanGroup
          key={c.spanId}
          node={c}
          depth={depth + 1}
          indexMap={indexMap}
          expanded={expanded}
          onToggle={onToggle}
        />
      ))}
    </div>
  );
}

/** Build the forest + a global call-index map (depth-first: a span's own calls
 *  before its children) and render the roots. Global numbering lets a user
 *  reference "call #5" regardless of which span it sits in. */
function SpanForest({
  traces,
  expanded,
  onToggle,
}: {
  traces: LlmTrace[];
  expanded: string | null;
  onToggle: (id: string) => void;
}) {
  const forest = buildSpanForest(traces);
  const indexMap = new Map<string, number>();
  let counter = 0;
  const walk = (node: SpanNode) => {
    for (const t of node.traces) indexMap.set(t.id, counter++);
    for (const c of node.children) walk(c);
  };
  for (const root of forest) walk(root);
  return (
    <>
      {forest.map((root) => (
        <SpanGroup
          key={root.spanId}
          node={root}
          depth={0}
          indexMap={indexMap}
          expanded={expanded}
          onToggle={onToggle}
        />
      ))}
    </>
  );
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
            <span style={{ color: 'var(--danger)' }}>加载失败: {error}</span>
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
            {hasSpans(traces) ? (
              <SpanForest
                traces={traces}
                expanded={expanded}
                onToggle={(id) => setExpanded(expanded === id ? null : id)}
              />
            ) : (
              traces.map((t, i) => (
                <TraceRow
                  key={t.id}
                  trace={t}
                  index={i}
                  expanded={expanded === t.id}
                  onToggle={() => setExpanded(expanded === t.id ? null : t.id)}
                />
              ))
            )}
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
        <div style={{ display: 'flex', height: 6, borderRadius: 3, overflow: 'hidden', background: 'var(--surface-3)' }}>
          <div style={{ width: `${(ttfb / denom) * 100}%`, background: 'var(--accent)' }} title={`ttfb ${ttfb}ms`} />
          <div style={{ width: `${(stream / denom) * 100}%`, background: 'var(--success)' }} title={`stream ${stream}ms`} />
          {other > 0 && (
            <div style={{ width: `${(other / denom) * 100}%`, background: 'var(--text-tertiary)' }} title={`other ${other}ms`} />
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
        <span style={{ fontWeight: 600, color: isError ? 'var(--danger)' : 'inherit' }}>{title}</span>
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
          background: 'var(--surface-2)',
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
