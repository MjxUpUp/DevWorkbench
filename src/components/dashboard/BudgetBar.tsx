import { useDashboardStore } from '../../stores/dashboardStore';

export function BudgetBar() {
  const budget = useDashboardStore((s) => s.budget);

  // v3: pi.dev 配色——fill 用纯 accent 色（不用 active-stripe 双色硬切，
  // 因为进度条宽度随百分比变化，硬切段会出现在任意位置看起来像断裂）。
  // active-stripe 只用于固定宽度的装饰条（StatusBar）。
  // 超过 90% 显示超限警示（学 OpenHands budgeting enforcement）。
  const overBudget = budget.percentage > 90;
  const fillStyle = overBudget
    ? { width: `${Math.min(budget.percentage, 100)}%`, background: 'var(--danger)' }
    : { width: `${Math.min(budget.percentage, 100)}%`, background: 'var(--accent)' };

  return (
    <div className="budget-bar">
      <div className="budget-bar-header">
        <span className="budget-bar-title">月度预算</span>
        <span className="budget-bar-amount">
          ${budget.spent.toFixed(0)} / ${budget.total.toFixed(0)}
        </span>
      </div>
      <div className="budget-bar-track">
        <div className="budget-bar-fill" style={fillStyle} />
      </div>
      <div className="budget-bar-footer">
        <span className="budget-bar-percentage">{budget.percentage.toFixed(1)}% 已使用</span>
        {overBudget && (
          <span className="budget-bar-warning">⚠ 超 90% 将停止 dispatch</span>
        )}
      </div>
    </div>
  );
}
