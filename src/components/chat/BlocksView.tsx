import { useState } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { ChatStreamEvent } from '../../types';
import { IconCpu } from '../Icons';

interface BlocksViewProps {
  events: ChatStreamEvent[];
  running: boolean;
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
  return (
    <div className="chat-blocks">
      {waiting ? (
        <div className="chat-blocks-waiting">
          <span className="chat-blocks-waiting-text">等待模型响应</span>
          <span className="chat-blocks-cursor" aria-hidden="true" />
        </div>
      ) : (
        <>
          {events.map((ev, i) => (
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

function safeStringify(v: unknown): string {
  if (v === null || v === undefined) return '';
  try {
    return JSON.stringify(v, null, 2);
  } catch {
    return String(v);
  }
}
