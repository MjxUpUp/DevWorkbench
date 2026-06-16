import { StatCards } from '../dashboard/StatCards';
import { CostTrendChart } from '../dashboard/CostTrendChart';
import { BudgetBar } from '../dashboard/BudgetBar';
import { QualityHistory } from '../dashboard/QualityHistory';

/**
 * "使用统计" settings section — reuses the dashboard widgets (cost trend,
 * budget, quality history) so usage data lives in one place now that the
 * Dashboard is no longer a top-level view.
 */
export function UsageStatsSection() {
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
    </div>
  );
}
