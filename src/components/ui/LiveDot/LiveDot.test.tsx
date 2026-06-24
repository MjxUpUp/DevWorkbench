import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import { LiveDot } from './LiveDot';

describe('LiveDot', () => {
  it('renders a span with dot class', () => {
    const { container } = render(<LiveDot />);
    const dot = container.querySelector('span');
    expect(dot).not.toBeNull();
    expect(dot?.className).toMatch(/dot/);
  });

  it('applies default running status (with blink animation)', () => {
    const { container } = render(<LiveDot />);
    const dot = container.querySelector('span')!;
    // running 不应带 static 类
    expect(dot.className).toMatch(/running/);
    expect(dot.className).not.toMatch(/static/);
  });

  it('idle status removes animation (static class)', () => {
    const { container } = render(<LiveDot status="idle" />);
    const dot = container.querySelector('span')!;
    expect(dot.className).toMatch(/idle/);
    expect(dot.className).toMatch(/static/);
  });

  it('applies size class', () => {
    const { container } = render(<LiveDot size="lg" />);
    expect(container.querySelector('span')!.className).toMatch(/lg/);
  });

  it('is aria-hidden (decorative)', () => {
    const { container } = render(<LiveDot />);
    expect(container.querySelector('span')?.getAttribute('aria-hidden')).toBe('true');
  });
});
