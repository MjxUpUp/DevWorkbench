import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { BlocksView } from '../BlocksView';
import type { ChatStreamEvent } from '../../../types';

describe('BlocksView', () => {
  it('renders each block kind in order, one card per event', () => {
    const events: ChatStreamEvent[] = [
      { kind: 'text', content: 'hello world' },
      { kind: 'tool_use', name: 'Read', input: { file_path: '/a.txt' } },
      { kind: 'tool_result', content: 'file contents here', is_error: false },
      { kind: 'result', is_error: false, secs: 12 },
    ];
    const { container } = render(<BlocksView events={events} running={false} />);

    // Text block → Markdown
    expect(screen.getByText('hello world')).toBeInTheDocument();
    // Tool-use shows the tool name
    expect(screen.getByText('Read')).toBeInTheDocument();
    // Result shows status label + elapsed seconds
    expect(screen.getByText('完成')).toBeInTheDocument();
    expect(screen.getByText('12s')).toBeInTheDocument();
    // Exactly 4 .chat-block cards, and no caret when not running
    expect(container.querySelectorAll('.chat-block').length).toBe(4);
    expect(container.querySelector('.chat-blocks-cursor')).toBeNull();
  });

  it('shows a streaming caret while running', () => {
    const { container } = render(<BlocksView events={[{ kind: 'text', content: 'x' }]} running={true} />);
    expect(container.querySelector('.chat-blocks-cursor')).not.toBeNull();
  });

  it('marks a failed result block with the failed class', () => {
    render(<BlocksView events={[{ kind: 'result', is_error: true, secs: 3 }]} running={false} />);
    expect(screen.getByText('失败')).toBeInTheDocument();
    const result = document.querySelector('.chat-block-result');
    expect(result?.classList.contains('failed')).toBe(true);
    expect(result?.classList.contains('ok')).toBe(false);
  });

  it('expands tool_use input on click (collapsed by default)', () => {
    render(<BlocksView events={[{ kind: 'tool_use', name: 'Bash', input: { command: 'ls -la' } }]} running={false} />);

    // Collapsed by default → input JSON not in the document
    expect(screen.queryByText(/"command": "ls -la"/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button'));
    expect(screen.getByText(/"command": "ls -la"/)).toBeInTheDocument();
  });

  it('renders tool_use string input verbatim when expanded', () => {
    render(<BlocksView events={[{ kind: 'tool_use', name: 'Grep', input: 'pattern' }]} running={false} />);
    fireEvent.click(screen.getByRole('button'));
    expect(screen.getByText('pattern')).toBeInTheDocument();
  });

  it('renders a thinking block as a collapsible card, collapsed by default', () => {
    // GLM interleaved reasoning arrives as a Thinking wire event; BlocksView
    // must render it under the thinking card class (distinct from tool/result
    // cards) so the UI reads it as auxiliary reasoning, not model output.
    const { container } = render(
      <BlocksView events={[{ kind: 'thinking', content: 'let me reason about this' }]} running={false} />,
    );
    expect(container.querySelector('.chat-block-thinking')).not.toBeNull();
    expect(screen.getByText('思考过程')).toBeInTheDocument();
    // Collapsed by default → the trace is not yet in the document.
    expect(screen.queryByText('let me reason about this')).not.toBeInTheDocument();
  });

  it('expands the thinking trace on click', () => {
    render(<BlocksView events={[{ kind: 'thinking', content: 'step by step plan' }]} running={false} />);
    fireEvent.click(screen.getByRole('button'));
    expect(screen.getByText('step by step plan')).toBeInTheDocument();
  });

  it('merges consecutive per-delta thinking events into a single card', () => {
    // GLM streams reasoning as many thinking_delta SSE chunks; the finalized
    // replay path (session.blocks from the DB) stores each as a separate event,
    // bypassing agentStore.appendBlock's live merge. Without render-layer
    // normalization one trace exploded into N stacked "思考过程" cards (the
    // acceptance symptom). normalizeEvents folds consecutive same-kind events.
    const { container } = render(
      <BlocksView
        events={[
          { kind: 'thinking', content: 'step 1. ' },
          { kind: 'thinking', content: 'step 2. ' },
          { kind: 'thinking', content: 'step 3.' },
        ]}
        running={false}
      />,
    );
    // Exactly ONE thinking card, not three.
    expect(container.querySelectorAll('.chat-block-thinking').length).toBe(1);
    fireEvent.click(screen.getByRole('button'));
    expect(screen.getByText('step 1. step 2. step 3.')).toBeInTheDocument();
  });

  it('does NOT merge thinking across a different-kind block in between', () => {
    // A text answer between two thinking traces means two separate reasoning
    // spans (interleaved thinking) — keep them as two cards, in order.
    const { container } = render(
      <BlocksView
        events={[
          { kind: 'thinking', content: 'first' },
          { kind: 'text', content: 'answer' },
          { kind: 'thinking', content: 'second' },
        ]}
        running={false}
      />,
    );
    expect(container.querySelectorAll('.chat-block-thinking').length).toBe(2);
  });

  it('merges consecutive text events the same way', () => {
    // Same per-delta fragmentation hits text blocks too; normalizeEvents covers
    // both text and thinking so a streamed Markdown reply stays one card.
    const { container } = render(
      <BlocksView
        events={[
          { kind: 'text', content: 'foo ' },
          { kind: 'text', content: 'bar ' },
          { kind: 'text', content: 'baz' },
        ]}
        running={false}
      />,
    );
    expect(container.querySelectorAll('.chat-block-text').length).toBe(1);
    expect(screen.getByText('foo bar baz')).toBeInTheDocument();
  });

  it('marks an errored tool result', () => {
    render(<BlocksView events={[{ kind: 'tool_result', content: 'boom', is_error: true }]} running={false} />);
    expect(screen.getByText('工具错误')).toBeInTheDocument();
    const card = document.querySelector('.chat-block-toolresult');
    expect(card?.classList.contains('error')).toBe(true);
  });

  it('renders a waiting hint + caret for an empty running stream', () => {
    // Running but no block has arrived yet (e.g. model gateway holding its
    // response) → show a "等待模型响应" hint + the streaming caret. This is the
    // structured-agent running state that replaces the old terminal "等待输出"
    // box — the chat-blocks form stays the only display for claude/react_kernel.
    const { container } = render(<BlocksView events={[]} running={true} />);
    expect(container.querySelectorAll('.chat-block').length).toBe(0);
    expect(screen.getByText('等待模型响应')).toBeInTheDocument();
    expect(container.querySelector('.chat-blocks-cursor')).not.toBeNull();
  });

  it('renders nothing for an empty completed stream', () => {
    const { container } = render(<BlocksView events={[]} running={false} />);
    expect(container.querySelectorAll('.chat-block').length).toBe(0);
    expect(container.querySelector('.chat-blocks-cursor')).toBeNull();
  });
});
