import { useEffect } from 'react';
import type { Project } from '../../types';
import { useActivityStore } from '../../stores/activityStore';
import { ActivityItem } from '../ActivityItem';

interface TimelineTabProps {
  project: Project | null;
}

export function TimelineTab({ project }: TimelineTabProps) {
  const events = useActivityStore((s) => s.events);
  const loading = useActivityStore((s) => s.loading);
  const loadForProject = useActivityStore((s) => s.loadForProject);
  const loadRecent = useActivityStore((s) => s.loadRecent);

  useEffect(() => {
    if (project) {
      loadForProject(project.path);
    } else {
      loadRecent(100);
    }
  }, [project, loadForProject, loadRecent]);

  if (loading && events.length === 0) {
    return <div className="tab-content-empty"><p>加载中...</p></div>;
  }

  if (events.length === 0) {
    return (
      <div className="tab-content-empty">
        <div className="tab-content-empty-icon">◷</div>
        <h2>暂无活动</h2>
        <p>完成一次 Agent 对话后将在此显示活动记录</p>
      </div>
    );
  }

  return (
    <div className="timeline-tab">
      {events.map((event) => (
        <ActivityItem key={event.id} event={event} />
      ))}
    </div>
  );
}
