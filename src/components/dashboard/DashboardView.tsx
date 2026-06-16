import { StatCards } from './StatCards';
import { CostTrendChart } from './CostTrendChart';
import { BudgetBar } from './BudgetBar';
import { QualityHistory } from './QualityHistory';

export function DashboardView() {
  return (
    <div className="dashboard-view">
      <div className="dashboard-header">
        <h2 className="dashboard-title">Dashboard</h2>
        <p className="dashboard-subtitle">成本与质量概览</p>
      </div>

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
