import { useState, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { invoke } from '@tauri-apps/api/core';
import type { ChatStreamEvent } from '../../types';
import { Frame } from '../ui/Frame/Frame';
import { L1Thinking } from './layers/L1Thinking';
import { L2ToolPill } from './layers/L2ToolPill';
import { WorkflowProgressStrip } from './WorkflowProgressStrip';
import styles from './BlocksView.module.css';

interface BlocksViewProps {
  events: ChatStreamEvent[];
  running: boolean;
  /** Session id this block stream belongs to. Required to resolve the compact
   *  card's expand action (loads dropped-message archive via
   *  read_compact_archive_cmd). Optional because the orchestrate canvas path
   *  renders BlocksView per-node without a session id — there the compact card
   *  shows its summary but the expand is disabled. */
  sessionId?: string;
}

/** Merge consecutive same-kind text/thinking events into one block before
 *  rendering. Live streaming already merges via agentStore.appendBlock, but the
 *  finalized replay path reads `session.blocks` from the DB which stores the
 *  raw per-delta stream (GLM emits one thinking_delta per SSE chunk) and
 *  bypasses that merge — without this, a single reasoning trace renders as N
 *  stacked "思考过程" cards, each holding a content fragment. Normalizing at
 *  the render layer fixes BOTH paths and is idempotent on already-merged data. */
function normalizeEvents(events: ChatStreamEvent[]): ChatStreamEvent[] {
  const out: ChatStreamEvent[] = [];
  for (const ev of events) {
    const last = out[out.length - 1];
    if (last && (ev.kind === 'text' || ev.kind === 'thinking') && last.kind === ev.kind) {
      (last as ChatStreamEvent & { content: string }).content += ev.content;
    } else {
      out.push({ ...ev });
    }
  }
  return out;
}

/** 一个 plan 步骤——块3（启示1「让 plan 可见」）。ReactKernel 的 ReactAgent 无显式
 *  plan 对象（plan 在 LLM context），故以 tool_use 为步骤锚点：前导 text/thinking 归
 *  入"开场"或当前步骤的思路，tool_result 标记步骤完成度，result/compact 独立收尾。 */
type StepKind = 'opening' | 'tool' | 'closing';
type StepStatus = 'running' | 'done' | 'error';
interface Step {
  key: string;
  kind: StepKind;
  /** tool 步骤的连续序号（1-based，仅 tool 步骤计数） */
  toolIdx?: number;
  toolName?: string;
  status: StepStatus;
  events: ChatStreamEvent[];
}

/** 把合并后的时序事件按 tool_use 切分成 plan 步骤。
 *  - 首个 tool_use 前的 text/thinking/file_changed → opening（开场说明）
 *  - 每个 tool_use 起一步，后续到下个 tool_use 前的非收尾事件归入该步
 *  - tool_result 落到最近 tool 步骤并标记完成度（done/error）
 *  - result/compact → 独立 closing 步骤（整个 turn 的收尾/压缩，不属任何 tool） */
function groupByStep(events: ChatStreamEvent[]): Step[] {
  const steps: Step[] = [];
  const opening: ChatStreamEvent[] = [];
  let currentTool: Step | null = null;
  let toolCounter = 0;
  let idx = 0;

  for (const ev of events) {
    if (ev.kind === 'result' || ev.kind === 'compact') {
      currentTool = null;
      steps.push({
        key: `close-${idx++}`,
        kind: 'closing',
        status: ev.kind === 'result' && ev.is_error ? 'error' : 'done',
        events: [ev],
      });
    } else if (ev.kind === 'tool_use') {
      currentTool = {
        key: `tool-${idx++}`,
        kind: 'tool',
        toolIdx: ++toolCounter,
        toolName: ev.name,
        status: 'running',
        events: [ev],
      };
      steps.push(currentTool);
    } else if (ev.kind === 'tool_result') {
      if (currentTool) {
        currentTool.events.push(ev);
        currentTool.status = ev.is_error ? 'error' : 'done';
      } else {
        // 孤儿 result（无前置 tool_use）——独立收尾展示
        steps.push({
          key: `orphan-${idx++}`,
          kind: 'closing',
          status: ev.is_error ? 'error' : 'done',
          events: [ev],
        });
      }
    } else {
      // text/thinking/file_changed：归当前 tool 步骤，否则攒到 opening
      if (currentTool) currentTool.events.push(ev);
      else opening.push(ev);
    }
  }
  if (opening.length) {
    steps.unshift({ key: `opening-${idx++}`, kind: 'opening', status: 'done', events: opening });
  }
  return steps;
}

/** Renders an agent's structured output as a stack of plan-step-grouped block
 *  cards — the chat-blocks UI for claude (and later ReactAgent).
 *
 * 块3（启示1）：事件流按 tool_use 切分成 plan 步骤，每步一个 StepGroup（步骤头标
 * 步骤号/tool/status + 左边框状态色），步骤内仍是 BlockCard。默认全展开——保留
 * E2E 的 chat-block-* 可见性，步骤头提供 plan 结构可见性（不再纯时序压扁）。
 * 步骤头为纯展示 div（不可折叠）——避免 button 嵌套 button 违反 HTML（步骤内的
 * tool_result L2ToolPill head 本身是 button）+ a11y 红线（design-refactor-complete）。 */
export function BlocksView({ events, running, sessionId }: BlocksViewProps) {
  const waiting = running && events.length === 0;
  const steps = useMemo(() => groupByStep(normalizeEvents(events)), [events]);
  return (
    <div className={styles.blocks} data-testid="chat-blocks">
      {waiting ? (
        <div className={styles.waiting}>
          <span className={styles.waitingText}>等待模型响应</span>
          <span className={styles.cursor} data-testid="chat-streaming-cursor" aria-hidden="true" />
        </div>
      ) : (
        <>
          {steps.map((step) => (
            <StepGroup key={step.key} step={step} running={running} sessionId={sessionId} />
          ))}
          {running && <span className={styles.cursor} data-testid="chat-streaming-cursor" aria-hidden="true" />}
        </>
      )}
    </div>
  );
}

const STATUS_ZH: Record<StepStatus, string> = {
  running: '运行中',
  done: '完成',
  error: '失败',
};

function StepGroup({ step, running, sessionId }: { step: Step; running: boolean; sessionId?: string }) {
  const label = step.kind === 'opening' ? '开场' : step.kind === 'closing' ? '收尾' : null;
  return (
    <div className={styles.stepGroup} data-status={step.status} data-kind={step.kind} data-testid="chat-step">
      <div className={styles.stepHead}>
        {step.kind === 'tool' ? (
          <>
            <span className={styles.stepIdx}>步骤 #{step.toolIdx}</span>
            <span className={styles.stepStatus} data-s={step.status}>{STATUS_ZH[step.status]}</span>
          </>
        ) : (
          <span className={styles.stepLabel}>{label}</span>
        )}
      </div>
      <div className={styles.stepBody}>
        {step.events.map((ev, i) => (
          <BlockCard key={i} event={ev} running={running} sessionId={sessionId} />
        ))}
      </div>
    </div>
  );
}

function BlockCard({ event, running, sessionId }: { event: ChatStreamEvent; running: boolean; sessionId?: string }) {
  switch (event.kind) {
    case 'text':
      return (
        <Frame variant="default" className={styles.textBlock} data-testid="chat-block-text">
          <div className={styles.textBody}>
            <ReactMarkdown remarkPlugins={[remarkGfm]}>{event.content}</ReactMarkdown>
          </div>
        </Frame>
      );
    case 'tool_use':
      // run_workflow_graph：tool_use pill 下挂实时节点状态条（orchestrator
      // 自规划 DAG 执行时逐节点点亮；图 settle 后 strip 自隐，交给
      // tool_result 的 format_outcome 文本作持久记录）。
      return event.name === 'run_workflow_graph' ? (
        <div>
          <ToolUsePill name={event.name} input={event.input} />
          <WorkflowProgressStrip />
        </div>
      ) : (
        <ToolUsePill name={event.name} input={event.input} />
      );
    case 'tool_result':
      return <ToolResultPill content={event.content} isError={event.is_error} />;
    case 'thinking':
      return (
        <L1Thinking
          summary={deriveThinkingSummary(event.content)}
          running={running}
          data-testid="chat-block-thinking"
        >
          {event.content}
        </L1Thinking>
      );
    case 'result':
      return (
        <div
          className={`${styles.result} ${event.is_error ? styles.failed : styles.ok}`}
          data-testid="chat-block-result"
        >
          <span className={styles.resultIcon}>{event.is_error ? '✗' : '✓'}</span>
          <span>{event.is_error ? '失败' : '完成'}</span>
          <span className={styles.resultSecs}>{event.secs}s</span>
        </div>
      );
    case 'file_changed':
      return (
        <div className={styles.file} data-testid="chat-block-file">
          <span className={styles.fileIcon} aria-hidden="true">📄</span>
          <span className={styles.filePath}>{event.path}</span>
        </div>
      );
    case 'compact':
      return <CompactCard event={event} sessionId={sessionId} />;
    case 'approval_required':
      // agentStore 短路 approval_required → ApprovalModal 弹窗（ApprovalModal.tsx:8），
      // 该块不进 blocks 数组，正常不可达。保留 case 仅为 switch 类型穷尽。
      return null;
    default: {
      // 穷尽检查：未来新增 ChatStreamEvent kind 时 TS 在此编译报错，强制显式
      // 处理，修现状「新事件种类静默丢」脆弱性（groundup-refactor-direction:36）。
      // 对齐 OrchestrateView.formatEvent 的 never default 模式。运行时兜底只在
      // 后端发了前端未声明的 kind（类型撒谎）时到达——渲染可见占位，不静默丢。
      const kindStr = (event as { kind: string }).kind;
      const _exhaustive: never = event;
      void _exhaustive;
      return (
        <div
          style={{ padding: 'var(--space-2)', fontSize: 'var(--text-xs)', color: 'var(--warning, var(--text-tertiary))' }}
          data-testid="chat-block-unknown"
        >
          未识别事件：{kindStr}
        </div>
      );
    }
  }
}

/** compact meta-event → 折叠摘要卡片。压缩发生时内核把被替换出模型历史的
 *  陈旧消息归档到 ~/.dev-workbench/agents/compact/{sid}.jsonl，并发出此事件。
 *  折叠态：一行摘要 + dropped 计数；展开态：异步 invoke 读 JSONL 原文（仅当
 *  有 sessionId；orchestrate canvas 路径无 sid 时禁用展开）。is_error 为熔断态
 *  （连续 3 次压缩失败 MAX_CONSECUTIVE_COMPACT_FAILURES）——渲染为危险卡片。 */
function CompactCard({ event, sessionId }: { event: Extract<ChatStreamEvent, { kind: 'compact' }>; sessionId?: string }) {
  const [expanded, setExpanded] = useState(false);
  const [archive, setArchive] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  // 展开只对有 sessionId 且非 error 的卡片有意义（error 态无归档可读）。
  const canExpand = !event.is_error && !!sessionId;

  async function handleExpand() {
    if (!canExpand || !sessionId) return;
    const next = !expanded;
    setExpanded(next);
    if (!next || archive !== null || loading) return;
    setLoading(true);
    setLoadError(null);
    try {
      const rows = await invoke<unknown[] | null>('read_compact_archive_cmd', { sessionId });
      setArchive(formatArchive(rows));
    } catch (err) {
      setLoadError(err instanceof Error ? err.message : String(err));
    } finally {
      setLoading(false);
    }
  }

  return (
    <div
      className={`${styles.compact}${event.is_error ? ` ${styles.isError}` : ''}`}
      data-testid="chat-block-compact"
    >
      <button
        type="button"
        className={styles.compactHeader}
        aria-expanded={expanded}
        disabled={!canExpand}
        onClick={handleExpand}
      >
        <span className={styles.compactIcon} aria-hidden="true">{event.is_error ? '⚠' : '🗜'}</span>
        <span className={styles.compactSummary}>{event.summary}</span>
        {event.dropped_count > 0 && (
          <span className={styles.compactCount}>-{event.dropped_count} msg</span>
        )}
        {canExpand && <span className={styles.compactToggle}>{expanded ? '▾' : '▸'}</span>}
      </button>
      {expanded && (
        <div className={styles.compactBody}>
          {loading && <span>加载归档…</span>}
          {loadError && <span>读取归档失败：{loadError}</span>}
          {!loading && !loadError && archive !== null && (
            <pre className={styles.compactArchive} data-testid="chat-block-compact-archive">{archive}</pre>
          )}
        </div>
      )}
    </div>
  );
}

/** 把 JSONL 归档行（{ts, kind, summary, dropped_count, dropped_messages}）格式化
 *  为可读文本。归档是调试/审计用途——等宽 pre 展示，不做 Markdown 渲染。接受
 *  null（read_compact_archive_cmd 返回 None 当归档文件不存在时）。 */
function formatArchive(rows: unknown[] | null): string {
  if (!Array.isArray(rows) || rows.length === 0) return '（无归档记录）';
  return rows
    .map((row, i) => {
      const r = row as Record<string, unknown>;
      const kind = typeof r.kind === 'string' ? r.kind : '?';
      const summary = typeof r.summary === 'string' ? r.summary : '';
      const count = typeof r.dropped_count === 'number' ? r.dropped_count : 0;
      const ts = typeof r.ts === 'string' ? r.ts : '';
      const head = `#${i + 1} [${kind}] ${ts} · dropped ${count}\n${summary}`;
      const msgs = Array.isArray(r.dropped_messages) ? r.dropped_messages : [];
      if (msgs.length === 0) return head;
      const body = msgs
        .map((m) => {
          const msg = m as Record<string, unknown>;
          const role = typeof msg.role === 'string' ? msg.role : '?';
          const content = typeof msg.content === 'string' ? msg.content : JSON.stringify(msg.content ?? '');
          return `  - (${role}) ${truncate(content, 160)}`;
        })
        .join('\n');
      return `${head}\n${body}`;
    })
    .join('\n\n');
}

/** tool_use → L2ToolPill（running 态，等配对的 tool_result 到达后转 success/error）*/
function ToolUsePill({ name, input }: { name: string; input: unknown }) {
  const inputStr = typeof input === 'string' ? input : safeStringify(input);
  return (
    <L2ToolPill
      name={name}
      desc={deriveToolDesc(name, input)}
      status="running"
      meta="调用中"
      nameTestId="chat-block-tool-name"
      data-testid="chat-block-tool"
    >
      <pre className={styles.toolInput} data-testid="chat-block-tool-input">{inputStr}</pre>
    </L2ToolPill>
  );
}

/** tool_result → L2ToolPill（success/error 态）*/
function ToolResultPill({ content, isError }: { content: string; isError: boolean }) {
  return (
    <L2ToolPill
      name={isError ? 'tool_error' : 'tool_result'}
      desc={truncate(content, 60)}
      status={isError ? 'error' : 'success'}
      meta={isError ? '失败' : '完成'}
      headTestId="chat-block-toolresult-head"
      data-testid="chat-block-toolresult"
    >
      <pre className={styles.toolResultContent} data-testid="chat-block-toolresult-content">{content}</pre>
    </L2ToolPill>
  );
}

/** 从 thinking content 提炼一行摘要（取首句或前 80 字）。*/
function deriveThinkingSummary(content: string): string {
  const firstLine = content.split('\n')[0]?.trim() ?? '';
  if (firstLine.length <= 80) return firstLine || '思考中...';
  return firstLine.slice(0, 80) + '...';
}

/** 从 tool_use name + input 提炼一行描述。*/
function deriveToolDesc(name: string, input: unknown): string {
  if (name === 'run_workflow_graph') {
    // 自规划工作流：从 graph.nodes 计数，让用户一眼看到 DAG 规模。
    const graph = (input as Record<string, unknown> | null)?.graph;
    const nodes = (graph as Record<string, unknown> | null | undefined)?.nodes;
    const n = nodes && typeof nodes === 'object' ? Object.keys(nodes as Record<string, unknown>).length : 0;
    return n > 0 ? `自规划工作流 · ${n} 节点` : '自规划工作流';
  }
  if (typeof input === 'object' && input !== null) {
    const obj = input as Record<string, unknown>;
    // 常见字段：file_path / path / command / pattern
    const path = obj.file_path ?? obj.path ?? obj.command ?? obj.pattern;
    if (typeof path === 'string') return path;
  }
  return name;
}

function truncate(s: string, n: number): string {
  const oneLine = s.replace(/\n/g, ' ').trim();
  return oneLine.length <= n ? oneLine : oneLine.slice(0, n) + '...';
}

function safeStringify(v: unknown): string {
  if (v === null || v === undefined) return '';
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}
