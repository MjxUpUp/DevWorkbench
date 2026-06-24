import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { Frame } from './Frame';

describe('Frame', () => {
  it('renders children', () => {
    render(<Frame>content</Frame>);
    expect(screen.getByText('content')).toBeInTheDocument();
  });

  it('applies default variant', () => {
    render(<Frame data-testid="f">x</Frame>);
    expect(screen.getByTestId('f').getAttribute('data-frame-variant')).toBe('default');
  });

  it('applies specified variant', () => {
    render(<Frame variant="highlight" data-testid="f">x</Frame>);
    expect(screen.getByTestId('f').getAttribute('data-frame-variant')).toBe('highlight');
  });

  it('renders 4 corner marks by default', () => {
    const { container } = render(<Frame>x</Frame>);
    // 4 个 aria-hidden 的 corner span
    const corners = container.querySelectorAll('[aria-hidden="true"]');
    expect(corners).toHaveLength(4);
  });

  it('hides corners when corners={false}', () => {
    const { container } = render(<Frame corners={false}>x</Frame>);
    const corners = container.querySelectorAll('[aria-hidden="true"]');
    expect(corners).toHaveLength(0);
  });

  it('forwards extra props', () => {
    render(<Frame id="my-frame" role="region">x</Frame>);
    expect(screen.getByRole('region')).toHaveAttribute('id', 'my-frame');
  });
});
