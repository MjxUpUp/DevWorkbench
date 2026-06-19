import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { SubagentBoard } from '../SubagentBoard';
import type { ChatStreamEvent } from '../../../types';

describe('SubagentBoard', () => {
  it('renders nothing when the stream has no dispatch_subagent calls', () => {
    const { container } = render(
      <SubagentBoard events={[{ kind: 'text', content: 'hi' }]} />,
    );
    expect(container.firstChild).toBeNull();
  });

  it('renders nothing for null/undefined events', () => {
    const { container } = render(<SubagentBoard events={null} />);
    expect(container.firstChild).toBeNull();
  });

  it('shows running + done counts and each dispatched task', () => {
    // Two dispatches fan out; only the first has resolved (done), the second
    // is still running — the board reflects a live 1-running / 1-done state.
    const events: ChatStreamEvent[] = [
      { kind: 'tool_use', name: 'dispatch_subagent', input: { task: '研究 A' } },
      { kind: 'tool_use', name: 'dispatch_subagent', input: { task: '研究 B' } },
      { kind: 'tool_result', content: '[子 agent 结论] done-a', is_error: false },
    ];
    render(<SubagentBoard events={events} />);
    expect(screen.getByLabelText('运行中子 agent 数').textContent).toBe('1 运行中');
    expect(screen.getByText('1 已完成')).toBeInTheDocument();
    expect(screen.getByText('研究 A')).toBeInTheDocument();
    expect(screen.getByText('研究 B')).toBeInTheDocument();
    // The resolved dispatch carries its terminal status as a data attribute.
    expect(screen.getByText('completed')).toBeInTheDocument();
    expect(screen.getByText('running')).toBeInTheDocument();
  });
});
