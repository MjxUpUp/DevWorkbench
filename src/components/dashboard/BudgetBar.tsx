import { useDashboardStore } from '../../stores/dashboardStore';

export function BudgetBar() {
  const budget = useDashboardStore((s) => s.budget);

  let barColor = 'var(--cost-safe)';
  if (budget.percentage > 80) {
    barColor = 'var(--cost-danger)';
  } else if (budget.percentage > 60) {
    barColor = 'var(--cost-warning)';
  }

  return (
    <div className="budget-bar">
      <div className="budget-bar-header">
        <span className="budget-bar-title">月度预算</span>
        <span className="budget-bar-amount">
          ${budget.spent.toFixed(0)} / ${budget.total.toFixed(0)}
        </span>
      </div>
      <div className="budget-bar-track">
        <div
          className="budget-bar-fill"
          style={{ width: `${Math.min(budget.percentage, 100)}%`, backgroundColor: barColor }}
        />
      </div>
      <div className="budget-bar-footer">
        <span className="budget-bar-percentage">{budget.percentage.toFixed(1)}% 已使用</span>
      </div>
    </div>
  );
}
