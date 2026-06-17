import { useState, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { ChatStreamEvent } from '../../types';
import { IconCpu } from '../Icons';

interface BlocksViewProps {
  events: ChatStreamEvent[];
  running: boolean;
}

/** Merge consecutive same-kind text/thinking events into one block before
 *  rendering. Live streaming already merges via agentStore.appendBlock, but the
 *  finalized replay path reads `session.blocks` from the DB which stores the
 *  raw per-delta stream (GLM emits one thinking_delta per SSE chunk) and
 *  bypasses that merge — without this, a single reasoning trace renders as N
 *  stacked "思考过程" cards, each holding a content fragment (the symptom in
 *  acceptance). Normalizing at the render layer fixes BOTH paths and is
 *  idempotent on already-merged live data. */
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
 *  (collapsible output, red on error), result (final status line). Raw agents
 *  (pi) emit no agent:event and never reach this view. */
export function BlocksView({ events, running }: BlocksViewProps) {
  // Structured agents (claude/react_kernel) reach this view even while running
  // with zero blocks yet — e.g. the model gateway is holding its response. Show
  // a chat-blocks-native "waiting" hint instead of falling back to the terminal
  // box, so the chat-blocks form is the ONLY display for structured agents.
  const waiting = running && events.length === 0;
  const merged = useMemo(() => normalizeEvents(events), [events]);
  return (
    <div className="chat-blocks">
      {waiting ? (
        <div className="chat-blocks-waiting">
          <span className="chat-blocks-waiting-text">等待模型响应</span>
          <span className="chat-blocks-cursor" aria-hidden="true" />
        </div>
      ) : (
        <>
          {merged.map((ev, i) => (
            <BlockCard key={i} event={ev} />
          ))}
          {running && <span className="chat-blocks-cursor" aria-hidden="true" />}
        </>
      )}
    </div>
  );
}

function BlockCard({ event }: { event: ChatStreamEvent }) {
  switch (event.kind) {
    case 'text':
      return (
        <div className="chat-block chat-block-text">
          <ReactMarkdown remarkPlugins={[remarkGfm]}>{event.content}</ReactMarkdown>
        </div>
      );
    case 'tool_use':
      return <ToolUseCard name={event.name} input={event.input} />;
    case 'tool_result':
      return <ToolResultCard content={event.content} isError={event.is_error} />;
    case 'thinking':
      return <ThinkingCard content={event.content} />;
    case 'result':
      return (
        <div className={`chat-block chat-block-result ${event.is_error ? 'failed' : 'ok'}`}>
          <span className="chat-block-result-icon">{event.is_error ? '✗' : '✓'}</span>
          <span>{event.is_error ? '失败' : '完成'}</span>
          <span className="chat-block-result-secs">{event.secs}s</span>
        </div>
      );
  }
}

function ToolUseCard({ name, input }: { name: string; input: unknown }) {
  const [open, setOpen] = useState(false);
  const inputStr = typeof input === 'string' ? input : safeStringify(input);
  return (
    <div className="chat-block chat-block-tool">
      <button type="button" className="chat-block-tool-head" onClick={() => setOpen((v) => !v)}>
        <IconCpu size={14} />
        <span className="chat-block-tool-name">{name}</span>
        <span className="chat-block-tool-toggle">{open ? '▾' : '▸'}</span>
      </button>
      {open && <pre className="chat-block-tool-input">{inputStr}</pre>}
    </div>
  );
}

function ToolResultCard({ content, isError }: { content: string; isError: boolean }) {
  const [open, setOpen] = useState(false);
  return (
    <div className={`chat-block chat-block-toolresult ${isError ? 'error' : ''}`}>
      <button type="button" className="chat-block-toolresult-head" onClick={() => setOpen((v) => !v)}>
        <span>{isError ? '✗' : '↳'}</span>
        <span>{isError ? '工具错误' : '工具结果'}</span>
        <span className="chat-block-tool-toggle">{open ? '▾' : '▸'}</span>
      </button>
      {open && <pre className="chat-block-toolresult-content">{content}</pre>}
    </div>
  );
}

function ThinkingCard({ content }: { content: string }) {
  // GLM interleaved thinking trace — collapsible, collapsed by default. Mirrors
  // the ToolResultCard shape but with a distinct class (muted/italic) so the
  // reasoning trace reads as auxiliary context, not model output. Collapsed by
  // default keeps a long trace from swamping the answer (same convention as
  // Claude/ChatGPT); open to inspect the reasoning.
  const [open, setOpen] = useState(false);
  return (
    <div className="chat-block chat-block-thinking">
      <button type="button" className="chat-block-thinking-head" onClick={() => setOpen((v) => !v)}>
        <span className="chat-block-thinking-mark" aria-hidden="true">✦</span>
        <span>思考过程</span>
        <span className="chat-block-tool-toggle">{open ? '▾' : '▸'}</span>
      </button>
      {open && <pre className="chat-block-thinking-content">{content}</pre>}
    </div>
  );
}

function safeStringify(v: unknown): string {
  if (v === null || v === undefined) return '';
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}
