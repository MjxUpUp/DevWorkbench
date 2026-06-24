import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { L2ToolPill } from './L2ToolPill';

describe('L2ToolPill', () => {
  it('renders pill with name + desc', () => {
    render(<L2ToolPill name="read_file" desc="foo.ts · 156 行">d</L2ToolPill>);
    expect(screen.getByText('read_file')).toBeInTheDocument();
    expect(screen.getByText(/foo\.ts/)).toBeInTheDocument();
  });

  it('shows ✓ icon for success status', () => {
    const { container } = render(<L2ToolPill name="x" desc="y" status="success" />);
    expect(container.textContent).toContain('✓');
  });

  it('shows ▸ for running, ✕ for error', () => {
    const { rerender, container } = render(<L2ToolPill name="x" desc="y" status="running" />);
    expect(container.textContent).toContain('▸');
    rerender(<L2ToolPill name="x" desc="y" status="error" />);
    expect(container.textContent).toContain('✕');
  });

  it('expands to show children on click', async () => {
    const user = userEvent.setup();
    render(<L2ToolPill name="x" desc="y">diff 内容</L2ToolPill>);
    const btn = screen.getByRole('button');
    await user.click(btn);
    expect(screen.getByText('diff 内容')).toBeInTheDocument();
  });

  it('hides chevron when no children', () => {
    render(<L2ToolPill name="x" desc="y" />);
    expect(screen.queryByText('›')).not.toBeInTheDocument();
  });

  it('shows meta when provided', () => {
    render(<L2ToolPill name="x" desc="y" meta="340ms · ckpt 2.1" />);
    expect(screen.getByText(/340ms/)).toBeInTheDocument();
  });
});
