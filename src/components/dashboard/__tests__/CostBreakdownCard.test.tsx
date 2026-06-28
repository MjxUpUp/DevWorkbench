import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
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

  // ── 单价换算 calculator ──
  // The what-if tool: re-estimate recorded tokens against any model's list
  // price. Estimate uses a "≈" marker + the recorded line carries "记录 " so
  // neither collides with the header's bare "$total" getByText assertions above.

  it('calculator shows a preset estimate + recorded cost for comparison', () => {
    useDashboardStore.setState({
      costSummary: {
        totalCost: 0.0042,
        totalInputTokens: 1_000_000,
        totalOutputTokens: 500_000,
        sessionCount: 1,
        inputCost: 0.003,
        outputCost: 0.0012,
      },
    });
    render(<CostBreakdownCard />);

    expect(screen.getByText('单价换算')).toBeInTheDocument();
    expect(screen.getByLabelText('模型单价预设')).toBeInTheDocument();
    // Default preset Claude Sonnet: 1M input @ $3 + 500k output @ $7.5 = $10.5.
    expect(screen.getByText(/≈ \$10\.5000/)).toBeInTheDocument();
    // Recorded cost shown for comparison when totalCost > 0.
    expect(screen.getByText(/记录 \$0\.0042/)).toBeInTheDocument();
  });

  it('recomputes the estimate when a different preset is chosen', () => {
    useDashboardStore.setState({
      costSummary: {
        totalCost: 0,
        totalInputTokens: 1_000_000,
        totalOutputTokens: 1_000_000,
        sessionCount: 1,
      },
    });
    render(<CostBreakdownCard />);

    // Initial (Sonnet): 1M×$3 + 1M×$15 = $18.
    expect(screen.getByText(/≈ \$18\.0000/)).toBeInTheDocument();

    // Switch to DeepSeek: 1M×$0.27 + 1M×$1.10 = $1.37.
    fireEvent.change(screen.getByLabelText('模型单价预设'), { target: { value: 'deepseek' } });
    expect(screen.getByText(/≈ \$1\.3700/)).toBeInTheDocument();
  });

  it('flips to 自定义 when a price is edited and uses the new value', () => {
    useDashboardStore.setState({
      costSummary: {
        totalCost: 0,
        totalInputTokens: 1_000_000,
        totalOutputTokens: 0,
        sessionCount: 1,
      },
    });
    render(<CostBreakdownCard />);

    const select = screen.getByLabelText('模型单价预设') as HTMLSelectElement;
    // Default Sonnet input @ $3 → $3 for 1M tokens.
    expect(screen.getByText(/≈ \$3\.0000/)).toBeInTheDocument();

    // Edit the input-unit-price to $10.
    const inputField = screen.getByLabelText('输入单价（美元/百万 tokens）');
    fireEvent.change(inputField, { target: { value: '10' } });

    // Selection flips to custom; estimate = 1M × $10 = $10.
    expect(select.value).toBe('custom');
    expect(screen.getByText(/≈ \$10\.0000/)).toBeInTheDocument();
  });

  it('renders the calculator when recorded cost is $0 but tokens exist', () => {
    // The UNKNOWN-pricing case the calculator exists for: backend recorded $0
    // (model not in pricing.rs) but real tokens were consumed.
    useDashboardStore.setState({
      costSummary: {
        totalCost: 0,
        totalInputTokens: 200_000,
        totalOutputTokens: 100_000,
        sessionCount: 1,
      },
    });
    render(<CostBreakdownCard />);
    expect(screen.getByText('单价换算')).toBeInTheDocument();
    // No recorded-cost comparison line when totalCost is 0.
    expect(screen.queryByText(/记录/)).not.toBeInTheDocument();
  });

  it('holds a mid-typing decimal like "3." without normalizing it away', () => {
    // The input is type="text" so the leading "3." of "$3.5" survives mid-typing
    // (a type="number" input would normalize it). The derived number is still 3.
    useDashboardStore.setState({
      costSummary: {
        totalCost: 0,
        totalInputTokens: 1_000_000,
        totalOutputTokens: 0,
        sessionCount: 1,
      },
    });
    render(<CostBreakdownCard />);

    const inputField = screen.getByLabelText('输入单价（美元/百万 tokens）') as HTMLInputElement;
    fireEvent.change(inputField, { target: { value: '3.' } });
    // Raw string preserved — not normalized to "3" and not jumped to "0".
    expect(inputField.value).toBe('3.');
    // Derived number = 3 (parseFloat), so 1M × $3 = $3 estimate still holds.
    expect(screen.getByText(/≈ \$3\.0000/)).toBeInTheDocument();
  });
});
