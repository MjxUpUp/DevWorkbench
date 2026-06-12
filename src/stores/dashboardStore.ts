import { create } from 'zustand';

export interface DashboardStats {
  todayCost: number;
  costTrend: number;       // percentage
  totalTokens: number;
  tokenTrend: number;
  activeSessions: number;
  qualityRate: number;
}

export interface CostTrendPoint {
  date: string;
  cost: number;
  tokens: number;
}

export interface BudgetInfo {
  spent: number;
  total: number;
  percentage: number;
}

export interface QualityEntry {
  sessionId: string;
  sessionNumber: number;
  score: number;       // X/9
  total: number;       // always 9
  agent: string;
  tokens: number;
  status: 'pass' | 'warn' | 'fail';
}

interface DashboardState {
  stats: DashboardStats;
  costTrend: CostTrendPoint[];
  budget: BudgetInfo;
  qualityHistory: QualityEntry[];
  loading: boolean;

  fetchDashboard: () => Promise<void>;
}

const MOCK_STATS: DashboardStats = {
  todayCost: 12.40,
  costTrend: 15,
  totalTokens: 245600,
  tokenTrend: 8,
  activeSessions: 18,
  qualityRate: 92,
};

const MOCK_COST_TREND: CostTrendPoint[] = [
  { date: 'Mon', cost: 8.20, tokens: 162000 },
  { date: 'Tue', cost: 11.50, tokens: 198000 },
  { date: 'Wed', cost: 9.80, tokens: 175000 },
  { date: 'Thu', cost: 14.30, tokens: 256000 },
  { date: 'Fri', cost: 10.60, tokens: 190000 },
  { date: 'Sat', cost: 7.40, tokens: 130000 },
  { date: 'Sun', cost: 12.40, tokens: 245600 },
];

const MOCK_BUDGET: BudgetInfo = {
  spent: 62,
  total: 80,
  percentage: 77.5,
};

const MOCK_QUALITY_HISTORY: QualityEntry[] = [
  { sessionId: 's-042', sessionNumber: 42, score: 9, total: 9, agent: 'claude-opus', tokens: 48200, status: 'pass' },
  { sessionId: 's-041', sessionNumber: 41, score: 7, total: 9, agent: 'claude-sonnet', tokens: 31500, status: 'warn' },
  { sessionId: 's-040', sessionNumber: 40, score: 9, total: 9, agent: 'claude-opus', tokens: 52100, status: 'pass' },
  { sessionId: 's-039', sessionNumber: 39, score: 4, total: 9, agent: 'gpt-4o', tokens: 28800, status: 'fail' },
  { sessionId: 's-038', sessionNumber: 38, score: 8, total: 9, agent: 'claude-sonnet', tokens: 35600, status: 'pass' },
  { sessionId: 's-037', sessionNumber: 37, score: 9, total: 9, agent: 'claude-opus', tokens: 44300, status: 'pass' },
  { sessionId: 's-036', sessionNumber: 36, score: 6, total: 9, agent: 'claude-sonnet', tokens: 22100, status: 'warn' },
  { sessionId: 's-035', sessionNumber: 35, score: 9, total: 9, agent: 'claude-opus', tokens: 51200, status: 'pass' },
];

export const useDashboardStore = create<DashboardState>((set) => ({
  stats: MOCK_STATS,
  costTrend: MOCK_COST_TREND,
  budget: MOCK_BUDGET,
  qualityHistory: MOCK_QUALITY_HISTORY,
  loading: false,

  fetchDashboard: async () => {
    set({ loading: true });
    // TODO: replace with real API calls when backend cost module is ready
    set({
      stats: MOCK_STATS,
      costTrend: MOCK_COST_TREND,
      budget: MOCK_BUDGET,
      qualityHistory: MOCK_QUALITY_HISTORY,
      loading: false,
    });
  },
}));
