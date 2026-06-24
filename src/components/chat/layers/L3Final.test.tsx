import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { L3Final } from './L3Final';

describe('L3Final', () => {
  it('renders title + body', () => {
    render(<L3Final title="重构完成">正文内容</L3Final>);
    expect(screen.getByText('重构完成')).toBeInTheDocument();
    expect(screen.getByText('正文内容')).toBeInTheDocument();
  });

  it('shows DONE badge by default', () => {
    render(<L3Final title="x">b</L3Final>);
    expect(screen.getByText('DONE')).toBeInTheDocument();
  });

  it('shows RUNNING badge for running status', () => {
    render(<L3Final title="x" status="running">b</L3Final>);
    expect(screen.getByText('RUNNING')).toBeInTheDocument();
  });

  it('shows FAILED badge for error status', () => {
    render(<L3Final title="x" status="error">b</L3Final>);
    expect(screen.getByText('FAILED')).toBeInTheDocument();
  });

  it('respects custom badge override', () => {
    render(<L3Final title="x" badge="CUSTOM">b</L3Final>);
    expect(screen.getByText('CUSTOM')).toBeInTheDocument();
  });

  it('renders actions when provided', () => {
    render(
      <L3Final title="x" actions={<button>Apply</button>}>b</L3Final>
    );
    expect(screen.getByText('Apply')).toBeInTheDocument();
  });

  it('uses article semantic with aria-label', () => {
    render(<L3Final title="x">b</L3Final>);
    expect(screen.getByRole('article')).toHaveAttribute('aria-label', '最终结论');
  });
});
