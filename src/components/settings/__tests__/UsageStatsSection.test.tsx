import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import { UsageStatsSection } from '../UsageStatsSection';
import { useDashboardStore } from '../../../stores/dashboardStore';
import { useAgentStore } from '../../../stores/agentStore';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));
// CostTrendChart renders chart.js <Line>, which calls canvas getComputedStyle
// and breaks under jsdom. Stub it — this test targets the fetchDashboard
// wiring, not chart rendering.
vi.mock('react-chartjs-2', () => ({ Line: () => null as never }));

/**
 * A1 第一刀：fetchDashboard 之前全仓零调用方，store 永远停在 EMPTY_STATS
 * ($0.00 / 0k)。这里覆盖"UsageStatsSection mount → 触发 fetchDashboard → 拉到
 * 真实 cost_summary → store 填充 → StatCards 显示真实费用"的完整链路。
 */
describe('UsageStatsSection — fetchDashboard wiring', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Reset to EMPTY so we observe the store actually being populated.
    useDashboardStore.setState({
      stats: {
        todayCost: 0, costTrend: 0, totalTokens: 0,
        tokenTrend: 0, activeSessions: 0, qualityRate: 0,
      },
      costTrend: [],
      budget: { spent: 0, total: 0, percentage: 0 },
      qualityHistory: [],
      loading: false,
    });
    useAgentStore.setState({ sessions: [] });
    vi.mocked(invoke).mockImplementation((cmd) => {
      if (cmd === 'get_cost_summary')
        return Promise.resolve({
          totalCost: 1.23,
          totalInputTokens: 1000,
          totalOutputTokens: 2000,
          sessionCount: 3,
        });
      if (cmd === 'get_cost_trend')
        return Promise.resolve([
          { date: '2026-06-18', cost: 0.5, tokens: 100 },
          { date: '2026-06-19', cost: 0.73, tokens: 150 },
        ]);
      if (cmd === 'load_budget')
        return Promise.resolve({ monthlyBudgetUsd: 10, alertThreshold: 0.8 });
      if (cmd === 'get_quality_reports')
        return Promise.resolve([
          {
            id: 'q1',
            sessionId: 'react-1',
            checks: [{ name: 'c', status: 'passed', message: null }],
            overallStatus: 'passed',
            createdAt: '2026-06-19T00:00:00Z',
          },
        ]);
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
  });

  it('triggers fetchDashboard on mount, fanning out to get_cost_summary', async () => {
    render(<UsageStatsSection />);
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('get_cost_summary');
    });
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('get_cost_trend', { days: 7 });
    });
  });

  it('fills today cost + day-over-day trends from cost_trend (last point = today)', async () => {
    render(<UsageStatsSection />);
    // trend is ORDER BY date ASC; last point 0.73 is today. pctChange(0.5→0.73)=46,
    // pctChange(100→150)=50.
    await waitFor(() => {
      const stats = useDashboardStore.getState().stats;
      expect(stats.todayCost).toBe(0.73);
      expect(stats.costTrend).toBe(46);
      expect(stats.tokenTrend).toBe(50);
    });
  });

  it('renders the real today cost via StatCards (not $0.00)', async () => {
    render(<UsageStatsSection />);
    expect(await screen.findByText('$0.73')).toBeInTheDocument();
  });
  // activeSessions derivation is covered by dashboardStore.test (store logic);
  // this component test stays focused on wiring + rendering.
});
