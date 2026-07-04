import { useEffect } from 'react';
import { StatCards } from '../dashboard/StatCards';
import { CostTrendChart } from '../dashboard/CostTrendChart';
import { BudgetBar } from '../dashboard/BudgetBar';
import { CostBreakdownCard } from '../dashboard/CostBreakdownCard';
import { EvalPanel } from '../dashboard/EvalPanel';
import { useDashboardStore } from '../../stores/dashboardStore';

/**
 * "使用统计" settings section — reuses the dashboard widgets (cost trend,
 * budget, breakdown, eval) so usage data lives in one place now that the
 * Dashboard is no longer a top-level view. B5 adds the BYOK transparent cost
 * breakdown so a user can see input/output/cache spend split, not just a total.
 *
 * 注：原 quality_history 段（多 session gate 摘要）已删除 —— 唯一数据写入路径
 * `save_report` 来自已退役的 CLI agent PTY；eval 视角改走 EvalPanel P-V-F-A。
 */
export function UsageStatsSection() {
  const fetchDashboard = useDashboardStore((s) => s.fetchDashboard);

  // SettingsView mounts this section with key={active.id}, so this fires on
  // each entry into the "使用统计" tab. Without it fetchDashboard has zero
  // callers and the store stays at EMPTY_STATS ($0.00 / 0k forever) even
  // though DbCostSink is writing real GLM data to cost_records.
  useEffect(() => {
    fetchDashboard();
  }, [fetchDashboard]);

  return (
    <div className="dashboard-view">
      <StatCards />

      <div className="dashboard-charts-row">
        <div className="dashboard-chart-panel">
          <CostTrendChart />
        </div>
        <div className="dashboard-budget-panel">
          <BudgetBar />
        </div>
      </div>

      <CostBreakdownCard />

      <EvalPanel />
    </div>
  );
}
