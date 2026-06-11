import type { QualityReport } from '../types';

interface QualityBadgeProps {
  report: QualityReport | null | undefined;
}

export function QualityBadge({ report }: QualityBadgeProps) {
  if (!report) {
    return <span className="quality-badge quality-badge-none">—</span>;
  }

  const passed = report.checks.filter(c => c.status === 'passed').length;
  const failed = report.checks.filter(c => c.status === 'failed').length;

  if (report.overallStatus === 'passed') {
    return (
      <span className="quality-badge quality-badge-passed">
        ✓ Passed ({passed}/{report.checks.length})
      </span>
    );
  }

  return (
    <span className="quality-badge quality-badge-failed">
      ✗ Failed ({failed} failed, {passed} passed)
    </span>
  );
}
