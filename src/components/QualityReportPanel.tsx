import type { QualityReport } from '../types';

interface QualityReportPanelProps {
  report: QualityReport;
}

const STATUS_ICONS: Record<string, string> = {
  passed: '✓',
  failed: '✗',
  warning: '⚠',
  unknown: '?',
};

const STATUS_LABELS: Record<string, string> = {
  passed: '通过',
  failed: '失败',
  warning: '警告',
  unknown: '未知',
};

const CHECK_DISPLAY_NAMES: Record<string, string> = {
  compile: '编译检查',
  assertion: '断言检查',
  pipeline: '流水线检查',
  forge_gate: 'Forge 质量门禁',
  test: '测试',
  lint: '代码检查',
};

function getCheckDisplayName(name: string): string {
  return CHECK_DISPLAY_NAMES[name] || name;
}

export function QualityReportPanel({ report }: QualityReportPanelProps) {
  const overallPassed = report.overallStatus === 'passed';

  return (
    <div className="quality-report-panel">
      <div className={`quality-report-header ${overallPassed ? 'passed' : 'failed'}`}>
        <span className="quality-report-overall-icon">
          {overallPassed ? '✓' : '✗'}
        </span>
        <span className="quality-report-overall-text">
          {overallPassed ? '质量门禁通过' : '质量门禁未通过'}
        </span>
        <span className="quality-report-time">
          {new Date(report.createdAt).toLocaleTimeString()}
        </span>
      </div>
      <div className="quality-report-checks">
        {report.checks.map((check, i) => (
          <div key={i} className={`quality-check-item ${check.status}`}>
            <span className="quality-check-icon">
              {STATUS_ICONS[check.status] || STATUS_ICONS.unknown}
            </span>
            <span className="quality-check-name">
              {getCheckDisplayName(check.name)}
            </span>
            <span className={`quality-check-status ${check.status}`}>
              {STATUS_LABELS[check.status] || check.status}
            </span>
            {check.message && (
              <div className="quality-check-message">{check.message}</div>
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
