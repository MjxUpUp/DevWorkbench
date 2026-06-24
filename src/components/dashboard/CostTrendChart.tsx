import {
  Chart as ChartJS,
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Title,
  Tooltip,
  Filler,
} from 'chart.js';
import { Line } from 'react-chartjs-2';
import { useDashboardStore } from '../../stores/dashboardStore';

ChartJS.register(
  CategoryScale,
  LinearScale,
  PointElement,
  LineElement,
  Title,
  Tooltip,
  Filler,
);

export function CostTrendChart() {
  const costTrend = useDashboardStore((s) => s.costTrend);

  const data = {
    labels: costTrend.map((p) => p.date),
    datasets: [
      {
        label: '费用 ($)',
        data: costTrend.map((p) => p.cost),
        fill: true,
        borderColor: 'var(--accent)',
        backgroundColor: 'rgba(37, 99, 235, 0.08)',
        pointBackgroundColor: 'var(--accent)',
        pointBorderColor: 'var(--surface-0)',
        pointBorderWidth: 2,
        pointRadius: 4,
        pointHoverRadius: 6,
        tension: 0.3,
      },
    ],
  };

  const options = {
    responsive: true,
    maintainAspectRatio: false,
    plugins: {
      legend: { display: false },
      title: {
        display: true,
        text: '7 天费用趋势',
        color: 'var(--text-primary)',
        font: { size: 14, weight: 600 as const, family: 'var(--font-sans)' },
        padding: { bottom: 16 },
      },
      tooltip: {
        backgroundColor: 'var(--surface-0)',
        titleColor: 'var(--text-primary)',
        bodyColor: 'var(--text-secondary)',
        borderColor: 'var(--border)',
        borderWidth: 1,
        padding: 10,
        callbacks: {
          label: (ctx: { parsed: { y: number | null } }) => {
            const value = ctx.parsed.y ?? 0;
            return `$${value.toFixed(2)}`;
          },
        },
      },
    },
    scales: {
      x: {
        grid: { color: 'var(--border-subtle)' },
        ticks: { color: 'var(--text-tertiary)', font: { size: 12 } },
      },
      y: {
        grid: { color: 'var(--border-subtle)' },
        ticks: {
          color: 'var(--text-tertiary)',
          font: { size: 12 },
          callback: (value: string | number) => `$${value}`,
        },
        beginAtZero: true,
      },
    },
  };

  return (
    <div className="cost-trend-chart">
      <Line data={data} options={options} />
    </div>
  );
}
