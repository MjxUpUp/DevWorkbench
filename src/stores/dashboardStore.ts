import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { CostSummary, CostTrendPoint, BudgetSettings, DashboardStats, BudgetInfo } from '../types';
import { useAgentStore } from './agentStore';

interface DashboardState {
  stats: DashboardStats;
  costTrend: CostTrendPoint[];
  budget: BudgetInfo;
  /** B5: raw CostSummary kept so the transparent-cost card can show the
   *  per-tier (input/output/cache) USD + token split, which DashboardStats
   *  collapses away. null until the first successful fetch. */
  costSummary: CostSummary | null;
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
  costSummary: null,
  loading: false,

  fetchDashboard: async () => {
    set({ loading: true });
    try {
      // 注：quality_history 段（multiple-session summary list）已删除 ——
      // 唯一数据写入路径 `save_report` 来自已退役的 CLI agent PTY 路径
      // （src-tauri/src/agents/pty.rs:2340），ReactKernel 用户视角下表基本空；
      // 实际 quality 信息请看 EvalPanel P5/V1/F1（per-session 详情）和
      // AgentMessage.qualityReport（单 session gate 展示）。
      const [summary, trend, budgetSettings] = await Promise.all([
        invoke<CostSummary>('get_cost_summary'),
        invoke<CostTrendPoint[]>('get_cost_trend', { days: 7 }),
        invoke<BudgetSettings>('load_budget'),
      ]);

      // null 防御：get_cost_summary 等在无数据/E2E shim 场景可能返回 null，原代码
      // 直接解引用 summary.totalInputTokens 会抛（latent bug——GateBar 块4 挂载
      // fetchDashboard 暴露之）。任一为 null 时早返回保留空默认，不冒泡为 console.error。
      if (!summary || !trend || !budgetSettings) {
        set({ loading: false });
        return;
      }

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
        qualityRate: 0, // quality_history 段删除后无来源；保留字段供未来用
      };

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
        costSummary: summary,
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
