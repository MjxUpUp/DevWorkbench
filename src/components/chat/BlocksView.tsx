import { useState, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { ChatStreamEvent } from '../../types';
import { Frame } from '../ui/Frame/Frame';
import { L1Thinking } from './layers/L1Thinking';
import { L2ToolPill } from './layers/L2ToolPill';
import { WorkflowProgressStrip } from './WorkflowProgressStrip';
import styles from './BlocksView.module.css';

interface BlocksViewProps {
  events: ChatStreamEvent[];
  running: boolean;
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

/** Renders an agent's structured output as a stack of block cards — the
 *  chat-blocks UI for claude (and later ReactAgent). Each `agent:event` becomes
 *  one card: text (Markdown), tool_use (collapsible input), tool_result
 *  (collapsible output, red on error), result (final status line).
 *
 * v3 重构：用 L1Thinking / L2ToolPill 替换原 ThinkingCard / ToolUseCard+
 * ToolResultCard，落地 Cursor 3.0 / Codex app 三段折叠范式。 */
export function BlocksView({ events, running }: BlocksViewProps) {
  const waiting = running && events.length === 0;
  const merged = useMemo(() => normalizeEvents(events), [events]);
  return (
    <div className={styles.blocks} data-testid="chat-blocks">
      {waiting ? (
        <div className={styles.waiting}>
          <span className={styles.waitingText}>等待模型响应</span>
          <span className={styles.cursor} data-testid="chat-streaming-cursor" aria-hidden="true" />
        </div>
      ) : (
        <>
          {merged.map((ev, i) => (
            <BlockCard key={i} event={ev} running={running} />
          ))}
          {running && <span className={styles.cursor} data-testid="chat-streaming-cursor" aria-hidden="true" />}
        </>
      )}
    </div>
  );
}

function BlockCard({ event, running }: { event: ChatStreamEvent; running: boolean }) {
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
  }
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

// 兼容旧测试：保留默认导出的 useState 引用避免 tree-shake 警告（实际已不用）
void useState;
