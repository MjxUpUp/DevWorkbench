import { useDashboardStore } from '../../stores/dashboardStore';
import type { QualityEntry } from '../../stores/dashboardStore';

const STATUS_ICONS: Record<QualityEntry['status'], { icon: string; className: string }> = {
  pass: { icon: '✅', className: 'quality-status-pass' },
  warn: { icon: '⚠️', className: 'quality-status-warn' },
  fail: { icon: '❌', className: 'quality-status-fail' },
};

export function QualityHistory() {
  const qualityHistory = useDashboardStore((s) => s.qualityHistory);

  return (
    <div className="quality-history">
      <h3 className="quality-history-title">质量门禁历史</h3>
      <div className="quality-history-list">
        {qualityHistory.map((entry) => {
          const { icon, className } = STATUS_ICONS[entry.status];
          return (
            <div key={entry.sessionId} className="quality-history-row">
              <span className={`quality-history-status ${className}`}>{icon}</span>
              <span className="quality-history-session">#{entry.sessionNumber}</span>
              <span className="quality-history-score">
                {entry.score}/{entry.total}
              </span>
              <span className="quality-history-agent">{entry.agent}</span>
              <span className="quality-history-tokens">
                {(entry.tokens / 1000).toFixed(1)}k
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
