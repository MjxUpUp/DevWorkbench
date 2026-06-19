import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { CostSummary, CostTrendPoint, BudgetSettings, QualityReport, DashboardStats, BudgetInfo, QualityEntry } from '../types';
import { useAgentStore } from './agentStore';

interface DashboardState {
  stats: DashboardStats;
  costTrend: CostTrendPoint[];
  budget: BudgetInfo;
  qualityHistory: QualityEntry[];
  loading: boolean;

  fetchDashboard: () => Promise<void>;
  saveBudget: (settings: BudgetSettings) => Promise<void>;
}

const EMPTY_STATS: DashboardStats = {
  todayCost: 0,
  costTrend: 0,
  totalTokens: 0,
  tokenTrend: 0,
  activeSessions: 0,
  qualityRate: 0,
};

const EMPTY_BUDGET: BudgetInfo = {
  spent: 0,
  total: 0,
  percentage: 0,
};

/** Day-over-day percent change. 0→N is +100%, N→0 is -100%, 0→0 is 0. */
function pctChange(prev: number, curr: number): number {
  if (prev === 0) return curr === 0 ? 0 : 100;
  return Math.round(((curr - prev) / prev) * 100);
}

export const useDashboardStore = create<DashboardState>((set) => ({
  stats: EMPTY_STATS,
  costTrend: [],
  budget: EMPTY_BUDGET,
  qualityHistory: [],
  loading: false,

  fetchDashboard: async () => {
    set({ loading: true });
    try {
      const [summary, trend, budgetSettings, reports] = await Promise.all([
        invoke<CostSummary>('get_cost_summary'),
        invoke<CostTrendPoint[]>('get_cost_trend', { days: 7 }),
        invoke<BudgetSettings>('load_budget'),
        invoke<QualityReport[]>('get_quality_reports'),
      ]);

      // Map CostSummary + cost_trend → DashboardStats. cost_trend is
      // `ORDER BY date` ASC, so the last point is today / most-recent day.
      const totalTokens = summary.totalInputTokens + summary.totalOutputTokens;
      const last = trend[trend.length - 1];
      const prev = trend[trend.length - 2];
      const todayCost = last?.cost ?? summary.totalCost;
      const costTrendPct = last && prev ? pctChange(prev.cost, last.cost) : 0;
      const tokenTrendPct = last && prev ? pctChange(prev.tokens, last.tokens) : 0;
      const runningSessions = useAgentStore
        .getState()
        .sessions.filter((s) => s.status === 'running').length;
      const stats: DashboardStats = {
        todayCost,
        costTrend: costTrendPct,
        totalTokens,
        tokenTrend: tokenTrendPct,
        activeSessions: runningSessions,
        qualityRate: 0, // computed below
      };

      // Map QualityReport[] → QualityEntry[]
      const qualityHistory: QualityEntry[] = reports
        .sort((a, b) => b.createdAt.localeCompare(a.createdAt))
        .slice(0, 20)
        .map((report, idx) => {
          const passed = report.checks.filter(c => c.status === 'passed').length;
          const total = report.checks.length;
          const overall = report.overallStatus;
          const status: QualityEntry['status'] =
            overall === 'passed' ? 'pass' : overall === 'failed' ? 'fail' : 'warn';
          return {
            sessionId: report.sessionId,
            sessionNumber: reports.length - idx,
            score: passed,
            total,
            agent: report.sessionId.split('-')[0] || 'unknown',
            tokens: 0,
            status,
          };
        });

      // Compute quality rate from reports
      if (reports.length > 0) {
        const passedCount = reports.filter(r => r.overallStatus === 'passed').length;
        stats.qualityRate = Math.round((passedCount / reports.length) * 100);
      }

      // Map BudgetSettings → BudgetInfo
      const budgetTotal = budgetSettings.monthlyBudgetUsd ?? 0;
      const monthCost = summary.totalCost; // approximation; ideally query current month only
      const budget: BudgetInfo = {
        spent: monthCost,
        total: budgetTotal,
        percentage: budgetTotal > 0 ? (monthCost / budgetTotal) * 100 : 0,
      };

      set({
        stats,
        costTrend: trend,
        budget,
        qualityHistory,
        loading: false,
      });
    } catch (e) {
      console.error('Failed to fetch dashboard data:', e);
      set({ loading: false });
    }
  },

  saveBudget: async (settings: BudgetSettings) => {
    try {
      await invoke('save_budget', { settings });
    } catch (e) {
      console.error('Failed to save budget settings:', e);
    }
  },
}));
