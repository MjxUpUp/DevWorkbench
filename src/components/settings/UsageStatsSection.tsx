import { useEffect } from 'react';
import { StatCards } from '../dashboard/StatCards';
import { CostTrendChart } from '../dashboard/CostTrendChart';
import { BudgetBar } from '../dashboard/BudgetBar';
import { QualityHistory } from '../dashboard/QualityHistory';
import { EvalPanel } from '../dashboard/EvalPanel';
import { useDashboardStore } from '../../stores/dashboardStore';

/**
 * "使用统计" settings section — reuses the dashboard widgets (cost trend,
 * budget, quality history) so usage data lives in one place now that the
 * Dashboard is no longer a top-level view.
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

      <QualityHistory />

      <EvalPanel />
    </div>
  );
}
