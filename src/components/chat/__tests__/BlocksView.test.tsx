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
