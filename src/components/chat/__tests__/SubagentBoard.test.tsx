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

  it('shows per-dispatch + aggregate cost when results carry a C2 footer', () => {
    // The anti-"10× cost" payoff: a fan-out's per-child and total spend must be
    // legible at a glance. Two resolved dispatches, each with a cost footer;
    // the board rolls them up in the header and shows each child inline.
    const events: ChatStreamEvent[] = [
      { kind: 'tool_use', name: 'dispatch_subagent', input: { task: '研究 A' } },
      { kind: 'tool_use', name: 'dispatch_subagent', input: { task: '研究 B' } },
      {
        kind: 'tool_result',
        content: '[子 agent 结论] a\n\n📊 子 agent 用量: 1000→200 tok · $0.0044',
        is_error: false,
      },
      {
        kind: 'tool_result',
        content: '[子 agent 结论] b\n\n📊 子 agent 用量: 500→100 tok · $0.0022',
        is_error: false,
      },
    ];
    render(<SubagentBoard events={events} />);
    // Per-child attribution inline.
    expect(screen.getByText('1000→200 tok · $0.0044')).toBeInTheDocument();
    expect(screen.getByText('500→100 tok · $0.0022')).toBeInTheDocument();
    // Header aggregate: 1500 in / 300 out / $0.0066 total.
    const total = screen.getByLabelText('子 agent 合计成本');
    expect(total.textContent).toContain('1500→300 tok');
    expect(total.textContent).toContain('$0.0066');
  });
});
