import { useDashboardStore } from '../../stores/dashboardStore';

export function StatCards() {
  const stats = useDashboardStore((s) => s.stats);

  const cards = [
    {
      label: '今日费用',
      value: `$${stats.todayCost.toFixed(2)}`,
      trend: stats.costTrend,
    },
    {
      label: 'Token 消耗',
      value: `${(stats.totalTokens / 1000).toFixed(1)}k`,
      trend: stats.tokenTrend,
    },
    {
      label: '进行中 Sessions',
      value: `${stats.activeSessions}`,
      trend: null,
    },
    {
      label: '质量通过率',
      value: `${stats.qualityRate}%`,
      trend: null,
    },
  ];

  return (
    <div className="stat-cards">
      {cards.map((card) => (
        <div key={card.label} className="stat-card">
          <div className="stat-card-value">{card.value}</div>
          <div className="stat-card-label">{card.label}</div>
          {card.trend !== null && (
            <div className={`stat-card-trend ${card.trend >= 0 ? 'trend-up' : 'trend-down'}`}>
              <span className="trend-arrow">{card.trend >= 0 ? '↑' : '↓'}</span>
              <span>{Math.abs(card.trend)}%</span>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}
