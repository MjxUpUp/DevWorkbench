import { useEffect, useState, type CSSProperties, type ReactNode } from 'react';
import { useNavigationStore } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';
import { useTraceStore } from '../../stores/traceStore';
import type { ChatStreamEvent, LlmTrace } from '../../types';

/**
 * 完整会话链路观察 — Langfuse 式 trace 视图。一个 turn（session）展开成三段：
 *
 *   1. 概要：提问、模型、LLM 调用次数、token、人工审批计数。
 *   2. 决策链路（DecisionFlow）：从 session.blocks 还原 agent 的时序——
 *      💭思考 → 🔧工具选用 → 📤执行结果 → 📁文件变更 → 🗜压缩 → ✅/❌结束。
 *      每个节点可展开看 thinking 全文 / tool input / result content。
 *   3. LLM 调用明细：每一次 ChatModel HTTP 调用（A1 span 树组织），req/resp
 *      body + status + latency + token，是 0.8s "400 失败" turn 的端到端诊断。
 *
 * 数据源（零后端改动）：
 *   - blocks 来自 agentStore.sessions[id].blocks（traceSessionId 永远对应一个当前
 *     turn，前端已有，无需新 IPC）。
 *   - LLM 明细来自 traceStore.fetchTraces（list_llm_traces）。
 *   - 审批节点来自 traceStore.fetchVerdicts（list_verdicts gate='human-gate'）——
 *     approval_required 在 react_chat.rs 被过滤出 blocks 流，verdicts 表是审批的
 *     唯一真相源，所以这里独立成段，不强行穿插进 blocks 时序（blocks 无时间戳，
 *     无法精确对齐审批的触发位置——穿插会伪造时序）。
 */

// ---- 决策链路节点共享样式 ----

const nodeHeadStyle: CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 8,
  width: '100%',
  padding: '7px 12px',
  background: 'transparent',
  border: 'none',
  textAlign: 'left',
  font: 'inherit',
  color: 'inherit',
  cursor: 'pointer',
};
const nodeRowStyle: CSSProperties = {
  display: 'flex',
  alignItems: 'center',
  gap: 8,
  padding: '7px 12px',
  borderBottom: '1px solid var(--border)',
};
const nodeIconStyle: CSSProperties = { width: 20, textAlign: 'center', flexShrink: 0 };
const nodeLabelStyle: CSSProperties = {
  fontFamily: 'var(--font-mono)',
  fontSize: 'var(--text-xs)',
  color: 'var(--text-secondary)',
  flexShrink: 0,
};
const nodeSummaryStyle: CSSProperties = {
  color: 'var(--text-tertiary)',
  fontSize: 'var(--text-xs)',
  flex: 1,
  overflow: 'hidden',
  textOverflow: 'ellipsis',
  whiteSpace: 'nowrap',
};
const nodeDetailStyle: CSSProperties = {
  margin: 0,
  padding: '8px 12px 12px 40px',
  background: 'var(--surface-2)',
  color: 'var(--text-secondary)',
  fontSize: 'var(--text-xs)',
  whiteSpace: 'pre-wrap',
  wordBreak: 'break-word',
  maxHeight: 300,
  overflow: 'auto',
};

const truncate = (s: string, n: number): string => (s.length > n ? s.slice(0, n) + '…' : s);

/** tool_use.input 是 unknown；安全序列化，循环引用/BigInt 不崩。 */
const safeStringify = (input: unknown): string => {
  try {
    return typeof input === 'string' ? input : JSON.stringify(input, null, 2);
  } catch {
    return String(input);
  }
};

/** 段落小标题（概要/决策链路/审批/LLM 明细共用）。 */
function SectionHeader({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        padding: '6px 12px',
        background: 'var(--surface-1)',
        fontSize: 'var(--text-xs)',
        color: 'var(--text-tertiary)',
        textTransform: 'uppercase',
        letterSpacing: '0.06em',
        fontWeight: 600,
      }}
    >
      {children}
    </div>
  );
}

/** 决策链路的一个节点：一种 block kind 对应一种行。可展开的（思考/工具/结果/
 *  回答）用 button + aria-expanded；不可展开的（文件变更/压缩/结束）用静态行。
 *  error 态（tool_result.is_error / compact.is_error）用红色 label 标记。 */
function DecisionNode({ block }: { block: ChatStreamEvent }) {
  const [open, setOpen] = useState(false);
  switch (block.kind) {
    case 'thinking':
      return (
        <div style={{ borderBottom: '1px solid var(--border)' }}>
          <button type="button" style={nodeHeadStyle} aria-expanded={open} onClick={() => setOpen((v) => !v)}>
            <span style={nodeIconStyle}>💭</span>
            <span style={nodeLabelStyle}>思考</span>
            <span style={nodeSummaryStyle}>{truncate(block.content, 100)}</span>
            <span style={{ color: 'var(--text-tertiary)' }}>{open ? '▾' : '▸'}</span>
          </button>
          {open && <pre style={nodeDetailStyle}>{block.content}</pre>}
        </div>
      );
    case 'tool_use':
      return (
        <div style={{ borderBottom: '1px solid var(--border)' }}>
          <button type="button" style={nodeHeadStyle} aria-expanded={open} onClick={() => setOpen((v) => !v)}>
            <span style={nodeIconStyle}>🔧</span>
            <span style={nodeLabelStyle}>{block.name}</span>
            <span style={nodeSummaryStyle}>{truncate(safeStringify(block.input), 100)}</span>
            <span style={{ color: 'var(--text-tertiary)' }}>{open ? '▾' : '▸'}</span>
          </button>
          {open && <pre style={nodeDetailStyle}>{safeStringify(block.input)}</pre>}
        </div>
      );
    case 'tool_result':
      return (
        <div style={{ borderBottom: '1px solid var(--border)' }}>
          <button type="button" style={nodeHeadStyle} aria-expanded={open} onClick={() => setOpen((v) => !v)}>
            <span style={nodeIconStyle}>{block.is_error ? '⛔' : '📤'}</span>
            <span style={{ ...nodeLabelStyle, color: block.is_error ? 'var(--danger)' : 'var(--text-secondary)' }}>
              {block.is_error ? '工具错误' : '工具结果'}
            </span>
            <span style={nodeSummaryStyle}>{truncate(block.content, 100)}</span>
            <span style={{ color: 'var(--text-tertiary)' }}>{open ? '▾' : '▸'}</span>
          </button>
          {open && <pre style={nodeDetailStyle}>{block.content}</pre>}
        </div>
      );
    case 'text':
      return (
        <div style={{ borderBottom: '1px solid var(--border)' }}>
          <button type="button" style={nodeHeadStyle} aria-expanded={open} onClick={() => setOpen((v) => !v)}>
            <span style={nodeIconStyle}>💬</span>
            <span style={nodeLabelStyle}>回答</span>
            <span style={nodeSummaryStyle}>{truncate(block.content, 100)}</span>
            <span style={{ color: 'var(--text-tertiary)' }}>{open ? '▾' : '▸'}</span>
          </button>
          {open && <pre style={nodeDetailStyle}>{block.content}</pre>}
        </div>
      );
    case 'file_changed':
      return (
        <div style={nodeRowStyle}>
          <span style={nodeIconStyle}>📁</span>
          <span style={nodeLabelStyle}>文件变更</span>
          <span style={{ ...nodeSummaryStyle, color: 'var(--text-secondary)' }}>{block.path}</span>
        </div>
      );
    case 'compact':
      return (
        <div style={{ ...nodeRowStyle, background: block.is_error ? 'rgba(255,0,0,0.04)' : 'transparent' }}>
          <span style={nodeIconStyle}>🗜</span>
          <span style={{ ...nodeLabelStyle, color: block.is_error ? 'var(--danger)' : 'var(--text-secondary)' }}>
            {block.is_error ? '压缩失败' : '上下文压缩'}
          </span>
          <span style={nodeSummaryStyle}>
            {block.summary}
            {block.dropped_count > 0 ? `（丢弃 ${block.dropped_count} 条）` : ''}
          </span>
        </div>
      );
    case 'result':
      return (
        <div style={{ ...nodeRowStyle, fontWeight: 600 }}>
          <span style={nodeIconStyle}>{block.is_error ? '❌' : '✅'}</span>
          <span style={{ color: block.is_error ? 'var(--danger)' : 'var(--success)' }}>
            {block.is_error ? '失败' : '完成'} · {block.secs.toFixed(1)}s
          </span>
        </div>
      );
    case 'approval_required':
      // react_chat 过滤掉 approval_required（verdicts 表是审批真相源）；防御性 null。
      return null;
    default:
      return null;
  }
}

// ---- LLM 调用明细（HTTP 级）----

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
  const verdicts = useTraceStore((s) => s.verdicts);
  const loading = useTraceStore((s) => s.loading);
  const error = useTraceStore((s) => s.error);
  const fetchTraces = useTraceStore((s) => s.fetchTraces);
  const fetchVerdicts = useTraceStore((s) => s.fetchVerdicts);
  // blocks 源：traceSessionId 对应的 session（前端已有，无需新 IPC）。session 可能在
  // refreshSessions 间短暂为 null（刚切到 trace、sessions 还没 reload）——hasBlocks 守卫。
  const session = useAgentStore((s) =>
    traceSessionId ? s.sessions.find((x) => x.id === traceSessionId) ?? null : null,
  );
  const [expanded, setExpanded] = useState<string | null>(null);

  useEffect(() => {
    if (traceSessionId) {
      void fetchTraces(traceSessionId);
      void fetchVerdicts(traceSessionId);
    }
    // 切 session 重置展开态：trace.id 是 uuid 碰撞几乎不可能，但 expanded 语义应
    // 跟随当前 session，避免残留上次的展开行。
    setExpanded(null);
  }, [traceSessionId, fetchTraces, fetchVerdicts]);

  if (!traceSessionId) {
    return (
      <div className="chat-view">
        <div className="chat-empty">
          <h2>会话 Trace</h2>
          <p style={{ fontSize: 'var(--text-sm)', color: 'var(--text-tertiary)' }}>
            从某个会话的「🔍 Trace」按钮进入，查看该 turn 的完整链路：思考 → 工具选用 → 执行结果 → 决策，以及每一次 LLM HTTP 调用的明细。
          </p>
        </div>
      </div>
    );
  }

  const llmCalls = traces ?? [];
  const totalIn = llmCalls.reduce((s, t) => s + (t.input_tokens ?? 0), 0);
  const totalOut = llmCalls.reduce((s, t) => s + (t.output_tokens ?? 0), 0);
  const modelSet = new Set(llmCalls.map((t) => t.model));
  const blocks = session?.blocks ?? null;
  const humanGates = verdicts.filter((v) => v.gate === 'human-gate');
  const hasLlm = llmCalls.length > 0;
  const hasBlocks = !!blocks && blocks.length > 0;

  return (
    <div className="chat-view">
      <div className="agent-message">
        <div className="agent-message-header">
          <span className="agent-block-title">会话 Trace</span>
          <span style={{ color: 'var(--text-tertiary)', fontSize: 'var(--text-xs)' }}>
            session: {traceSessionId.slice(0, 8)}
          </span>
          <button type="button" className="agent-message-copy-id" onClick={() => setActiveView('task')}>
            ← 返回对话
          </button>
        </div>

        {/* 概要 */}
        <div style={{ padding: '8px 12px', borderBottom: '1px solid var(--border)', display: 'flex', flexDirection: 'column', gap: 4 }}>
          {session?.prompt && (
            <div style={{ display: 'flex', gap: 8, alignItems: 'baseline' }}>
              <span style={{ color: 'var(--text-tertiary)', fontSize: 'var(--text-xs)', flexShrink: 0 }}>📥 提问</span>
              <span style={{ color: 'var(--text-primary)', fontSize: 'var(--text-sm)', whiteSpace: 'pre-wrap', display: 'block', maxHeight: 60, overflow: 'auto' }}>{session.prompt}</span>
            </div>
          )}
          <div style={{ display: 'flex', gap: 12, flexWrap: 'wrap', color: 'var(--text-tertiary)', fontSize: 'var(--text-xs)', fontVariantNumeric: 'tabular-nums' }}>
            {modelSet.size > 0 && <span>{[...modelSet].join(' · ')}</span>}
            {hasLlm && <span>{llmCalls.length} 次 LLM 调用</span>}
            {(totalIn > 0 || totalOut > 0) && <span>{totalIn}/{totalOut} tok</span>}
            {session?.model && <span>model: {session.model}</span>}
            {humanGates.length > 0 && (
              <span style={{ color: 'var(--warning)' }}>⚠ {humanGates.length} 次人工审批</span>
            )}
          </div>
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

        {/* 决策链路（blocks 主线） */}
        {!loading && !error && blocks && blocks.length > 0 && (
          <div style={{ borderBottom: '1px solid var(--border)' }}>
            <SectionHeader>决策链路 · {blocks.length} 步</SectionHeader>
            {blocks.map((b, i) => (
              <DecisionNode key={`${i}-${b.kind}`} block={b} />
            ))}
          </div>
        )}

        {/* 人工审批记录 */}
        {!loading && !error && humanGates.length > 0 && (
          <div style={{ borderBottom: '1px solid var(--border)' }}>
            <SectionHeader>人工审批 · {humanGates.length} 次</SectionHeader>
            {humanGates.map((v) => {
              const approved = v.verdict.toLowerCase().includes('approv');
              return (
                <div key={v.id} style={nodeRowStyle}>
                  <span style={nodeIconStyle}>⚠</span>
                  <span style={{ ...nodeLabelStyle, color: approved ? 'var(--success)' : 'var(--danger)' }}>
                    {v.verdict}
                  </span>
                  {v.report && <span style={nodeSummaryStyle}>{v.report}</span>}
                </div>
              );
            })}
          </div>
        )}

        {/* LLM 调用明细 */}
        {!loading && !error && hasLlm && (
          <div>
            <SectionHeader>LLM 调用明细 · {llmCalls.length} 次</SectionHeader>
            <div className="agent-block-body" style={{ padding: 0 }}>
              {hasSpans(llmCalls) ? (
                <SpanForest
                  traces={llmCalls}
                  expanded={expanded}
                  onToggle={(id) => setExpanded(expanded === id ? null : id)}
                />
              ) : (
                llmCalls.map((t, i) => (
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
          </div>
        )}

        {/* 无任何可追溯数据 */}
        {!loading && !error && !hasLlm && !hasBlocks && (
          <div className="agent-block-body">
            <span style={{ color: 'var(--text-tertiary)' }}>
              该会话没有可追溯的链路数据。可能在首次请求前就失败，或为非内核 agent（CLI 路径不接 trace sink，blocks 也未持久化）。
            </span>
          </div>
        )}

        {/* blocks 未持久化的诚实提示 */}
        {!loading && !error && !hasBlocks && session && (
          <div className="agent-block-body" style={{ fontSize: 'var(--text-xs)', color: 'var(--text-tertiary)' }}>
            注：该会话的决策链路（思考/工具/结果 blocks）未持久化——可能是流式会话尚未 finalize，或为历史会话。
            {hasLlm ? '上方仍可见 LLM HTTP 调用明细。' : ''}
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
