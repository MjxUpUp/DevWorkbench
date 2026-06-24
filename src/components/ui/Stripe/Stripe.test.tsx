import { describe, it, expect } from 'vitest';
import { render } from '@testing-library/react';
import { Stripe } from './Stripe';

describe('Stripe', () => {
  it('renders a span with stripe class', () => {
    const { container } = render(<Stripe />);
    const s = container.querySelector('span');
    expect(s).not.toBeNull();
    expect(s?.className).toMatch(/stripe/);
  });

  it('applies height class', () => {
    const { container } = render(<Stripe height="lg" />);
    expect(container.querySelector('span')!.className).toMatch(/h-lg/);
  });

  it('applies width class', () => {
    const { container } = render(<Stripe width="auto" />);
    expect(container.querySelector('span')!.className).toMatch(/w-auto/);
  });

  it('defaults to md height + full width', () => {
    const { container } = render(<Stripe />);
    const cls = container.querySelector('span')!.className;
    expect(cls).toMatch(/h-md/);
    expect(cls).toMatch(/w-full/);
  });

  it('is aria-hidden', () => {
    const { container } = render(<Stripe />);
    expect(container.querySelector('span')?.getAttribute('aria-hidden')).toBe('true');
  });
});
