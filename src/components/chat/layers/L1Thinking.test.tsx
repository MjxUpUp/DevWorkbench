import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { L1Thinking } from './L1Thinking';

describe('L1Thinking', () => {
  it('renders collapsed by default with summary', () => {
    render(<L1Thinking secs={14} summary="分析了现状">详情内容</L1Thinking>);
    expect(screen.getByText(/THOUGHT FOR 14s/)).toBeInTheDocument();
    expect(screen.getByText(/分析了现状/)).toBeInTheDocument();
    // 折叠态不显示 body
    expect(screen.queryByText('详情内容')).not.toBeInTheDocument();
  });

  it('expands on click to show body', async () => {
    const user = userEvent.setup();
    render(<L1Thinking summary="摘要">详情内容</L1Thinking>);
    const btn = screen.getByRole('button');
    expect(btn).toHaveAttribute('aria-expanded', 'false');
    await user.click(btn);
    expect(btn).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('详情内容')).toBeInTheDocument();
  });

  it('shows tokens when provided', () => {
    render(<L1Thinking summary="x" tokens={842}>d</L1Thinking>);
    expect(screen.getByText(/842 tok/)).toBeInTheDocument();
  });

  it('respects defaultExpanded', () => {
    render(<L1Thinking summary="x" defaultExpanded>默认展开</L1Thinking>);
    expect(screen.getByRole('button')).toHaveAttribute('aria-expanded', 'true');
    expect(screen.getByText('默认展开')).toBeInTheDocument();
  });

  it('shows THINKING label when no secs', () => {
    render(<L1Thinking summary="x">d</L1Thinking>);
    expect(screen.getByText('THINKING')).toBeInTheDocument();
  });
});
