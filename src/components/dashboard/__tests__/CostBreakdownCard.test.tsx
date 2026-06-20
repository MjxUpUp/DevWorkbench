import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { CostBreakdownCard } from '../CostBreakdownCard';
import { useDashboardStore } from '../../../stores/dashboardStore';

/**
 * B5 — the BYOK transparent cost card. These tests drive it directly by setting
 * `costSummary` in the store (no fetch / IPC), covering the three contracts:
 *  1. renders the per-tier USD + token split from the breakdown fields,
 *  2. hides cache rows when the provider reported no cache usage (GLM), and
 *  3. renders nothing when there's no cost data yet.
 */
describe('CostBreakdownCard — transparent breakdown', () => {
  beforeEach(() => {
    useDashboardStore.setState({ costSummary: null });
  });

  it('renders each tier with its USD split + token count', () => {
    useDashboardStore.setState({
      costSummary: {
        totalCost: 22.05,
        totalInputTokens: 1_000_000,
        totalOutputTokens: 1_000_000,
        sessionCount: 1,
        totalCacheReadTokens: 1_000_000,
        totalCacheWriteTokens: 1_000_000,
        inputCost: 3.0,
        outputCost: 15.0,
        cacheReadCost: 0.3,
        cacheWriteCost: 3.75,
      },
    });

    render(<CostBreakdownCard />);

    // Header total.
    expect(screen.getByText('$22.0500')).toBeInTheDocument();
    // Per-tier USD (4dp).
    expect(screen.getByText('$3.0000')).toBeInTheDocument(); // input
    expect(screen.getByText('$15.0000')).toBeInTheDocument(); // output
    expect(screen.getByText('$0.3000')).toBeInTheDocument(); // cache read
    expect(screen.getByText('$3.7500')).toBeInTheDocument(); // cache write
    // Per-tier token counts.
    expect(screen.getAllByText('1,000,000 tokens').length).toBe(4);
    // Tier labels render.
    expect(screen.getByText('输入')).toBeInTheDocument();
    expect(screen.getByText('输出')).toBeInTheDocument();
    expect(screen.getByText('缓存读取')).toBeInTheDocument();
    expect(screen.getByText('缓存写入')).toBeInTheDocument();
    // The honest BYOK note.
    expect(screen.getByText(/BYOK 透明成本/)).toBeInTheDocument();
  });

  it('hides cache rows when the provider reported no cache usage (GLM)', () => {
    // GLM folds cache into input_tokens and reports no cache tiers → the card
    // must NOT show empty $0 cache bars that would imply a feature that isn't
    // there. Only input + output tiers render.
    useDashboardStore.setState({
      costSummary: {
        totalCost: 0.0026,
        totalInputTokens: 1000,
        totalOutputTokens: 500,
        sessionCount: 1,
        totalCacheReadTokens: 0,
        totalCacheWriteTokens: 0,
        inputCost: 0.001,
        outputCost: 0.0016,
        cacheReadCost: 0,
        cacheWriteCost: 0,
      },
    });

    render(<CostBreakdownCard />);

    expect(screen.getByText('输入')).toBeInTheDocument();
    expect(screen.getByText('输出')).toBeInTheDocument();
    expect(screen.queryByText('缓存读取')).not.toBeInTheDocument();
    expect(screen.queryByText('缓存写入')).not.toBeInTheDocument();
  });

  it('renders nothing when there is no cost data yet', () => {
    // Fresh app / zero model calls → costSummary null.
    const { container } = render(<CostBreakdownCard />);
    expect(container).toBeEmptyDOMElement();
  });

  it('renders nothing when total cost is zero', () => {
    // A summary present but $0 (e.g. all UNKNOWN-pricing models) → no card.
    useDashboardStore.setState({
      costSummary: {
        totalCost: 0,
        totalInputTokens: 0,
        totalOutputTokens: 0,
        sessionCount: 0,
      },
    });
    const { container } = render(<CostBreakdownCard />);
    expect(container).toBeEmptyDOMElement();
  });
});
