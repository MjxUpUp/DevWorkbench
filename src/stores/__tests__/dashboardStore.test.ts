import { describe, it, expect, vi, beforeEach } from 'vitest';
import { useDashboardStore } from '../dashboardStore';
import { useAgentStore } from '../agentStore';
import { invoke } from '@tauri-apps/api/core';
import type { Session, SessionStatus } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

/** Build a type-complete Session so mock data stays valid if Session grows
 *  — avoids `as never` type-escape that would silently pass on schema drift. */
function makeSession(id: string, status: SessionStatus): Session {
  return {
    id,
    projectPath: '/proj',
    agentType: 'react_kernel',
    status,
    prompt: '',
    model: null,
    startedAt: '2026-06-19T00:00:00Z',
    finishedAt: null,
    exitCode: null,
    outputSummary: null,
    contextSnapshot: null,
    linkedRequirementId: null,
    parentSessionId: null,
    conversationId: null,
  };
}

/**
 * A1 第一刀：fetchDashboard 的字段映射——todayCost 取 cost_trend 末日（语义
 * 修正，原误用 summary.totalCost）；costTrend/tokenTrend 用 pctChange 算环比；
 * activeSessions 从 agentStore running sessions 派生。
 */
describe('dashboardStore.fetchDashboard — field mapping', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useAgentStore.setState({ sessions: [] });
    vi.mocked(invoke).mockImplementation((cmd) => {
      if (cmd === 'get_cost_summary')
        return Promise.resolve({
          totalCost: 5,
          totalInputTokens: 1000,
          totalOutputTokens: 2000,
          sessionCount: 3,
        });
      if (cmd === 'get_cost_trend')
        return Promise.resolve([
          { date: '2026-06-18', cost: 100, tokens: 1000 },
          { date: '2026-06-19', cost: 150, tokens: 1200 },
        ]);
      if (cmd === 'load_budget')
        return Promise.resolve({ monthlyBudgetUsd: 500, alertThreshold: 0.8 });
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
  });

  it('maps cost_trend last point to todayCost + day-over-day pct', async () => {
    await useDashboardStore.getState().fetchDashboard();
    const stats = useDashboardStore.getState().stats;
    expect(stats.todayCost).toBe(150); // last point, not summary.totalCost(5)
    expect(stats.costTrend).toBe(50);  // 100→150 = +50%
    expect(stats.tokenTrend).toBe(20); // 1000→1200 = +20%
    expect(stats.totalTokens).toBe(3000);
  });

  it('pctChange handles zero baselines: 0→N is +100%, 0→0 stays 0', async () => {
    vi.mocked(invoke).mockImplementation((cmd) => {
      if (cmd === 'get_cost_summary')
        return Promise.resolve({ totalCost: 0, totalInputTokens: 0, totalOutputTokens: 0, sessionCount: 0 });
      if (cmd === 'get_cost_trend')
        return Promise.resolve([
          { date: '2026-06-18', cost: 0, tokens: 0 },
          { date: '2026-06-19', cost: 50, tokens: 0 },
        ]);
      if (cmd === 'load_budget')
        return Promise.resolve({ monthlyBudgetUsd: null, alertThreshold: 0.8 });
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    await useDashboardStore.getState().fetchDashboard();
    const stats = useDashboardStore.getState().stats;
    expect(stats.costTrend).toBe(100); // 0→50 = +100%
    expect(stats.tokenTrend).toBe(0);  // 0→0 = 0
    expect(stats.todayCost).toBe(50);
  });

  it('falls back to summary.totalCost and 0 trend when trend is empty', async () => {
    vi.mocked(invoke).mockImplementation((cmd) => {
      if (cmd === 'get_cost_summary')
        return Promise.resolve({
          totalCost: 5,
          totalInputTokens: 0,
          totalOutputTokens: 0,
          sessionCount: 0,
        });
      if (cmd === 'get_cost_trend') return Promise.resolve([]);
      if (cmd === 'load_budget')
        return Promise.resolve({ monthlyBudgetUsd: null, alertThreshold: 0.8 });
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    await useDashboardStore.getState().fetchDashboard();
    expect(useDashboardStore.getState().stats.todayCost).toBe(5);
    expect(useDashboardStore.getState().stats.costTrend).toBe(0);
  });

  it('counts running sessions from agentStore', async () => {
    useAgentStore.setState({
      sessions: [
        makeSession('a', 'running'),
        makeSession('b', 'running'),
        makeSession('c', 'failed'),
      ],
    });
    await useDashboardStore.getState().fetchDashboard();
    expect(useDashboardStore.getState().stats.activeSessions).toBe(2);
  });
});
