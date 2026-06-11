import { useMemo } from 'react';
import type { Project } from '../../types';
import { useAgentStore } from '../../stores/agentStore';
import { QualityBadge } from '../QualityBadge';

interface OverviewTabProps {
  project: Project | null;
}

export function OverviewTab({ project }: OverviewTabProps) {
  const sessions = useAgentStore((s) => s.sessions);
  const requirements = useAgentStore((s) => s.requirements);
  const qualityReports = useAgentStore((s) => s.qualityReports);

  // ALL hooks must run before any conditional return
  const latestReport = useMemo(() => {
    if (!project) return null;
    const projectSessions = sessions.filter((s) => s.projectPath === project.path);
    const reports = projectSessions
      .filter(s => s.status === 'completed' || s.status === 'failed')
      .flatMap(s => {
        const r = qualityReports.get(s.id);
        return r ? [r] : [];
      })
      .sort((a, b) => b.createdAt.localeCompare(a.createdAt));
    return reports[0] ?? null;
  }, [project, sessions, qualityReports]);

  if (!project) {
    return (
      <div className="tab-content-empty">
        <div className="tab-content-empty-icon">DW</div>
        <h2>Dev Workbench</h2>
        <p>从左侧选择项目开始工作</p>
      </div>
    );
  }

  const projectSessions = sessions.filter((s) => s.projectPath === project.path);
  const projectRequirements = requirements.filter((r) => r.projectPath === project.path);
  const running = projectSessions.filter((s) => s.status === 'running').length;
  const completed = projectSessions.filter((s) => s.status === 'completed').length;
  const failed = projectSessions.filter((s) => s.status === 'failed').length;
  const openReqs = projectRequirements.filter((r) => r.status === 'todo').length;

  return (
    <div className="overview-tab">
      <div className="overview-header">
        <h2 className="overview-project-name">{project.name}</h2>
        <p className="overview-project-path">{project.path}</p>
      </div>

      <div className="overview-stats">
        <div className="overview-stat-card">
          <span className="overview-stat-value">{running}</span>
          <span className="overview-stat-label">运行中</span>
        </div>
        <div className="overview-stat-card">
          <span className="overview-stat-value">{completed}</span>
          <span className="overview-stat-label">已完成</span>
        </div>
        <div className="overview-stat-card">
          <span className="overview-stat-value">{failed}</span>
          <span className="overview-stat-label">失败</span>
        </div>
        <div className="overview-stat-card">
          <span className="overview-stat-value">{openReqs}</span>
          <span className="overview-stat-label">待处理</span>
        </div>
        <div className="overview-stat-card quality-card">
          <QualityBadge report={latestReport} />
          <span className="overview-stat-label">质量</span>
        </div>
      </div>

      <div className="overview-tags">
        {project.tags.map((tag) => (
          <span key={tag} className="overview-tag">{tag}</span>
        ))}
      </div>
    </div>
  );
}
