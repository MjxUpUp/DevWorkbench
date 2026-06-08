import type { Project, GitStatus, Session, AgentInfo, Requirement } from '../types';
import { ProjectCard } from './ProjectCard';
import { IconInbox } from './Icons';

interface ProjectGridProps {
  projects: Project[];
  gitStatusMap: Record<string, GitStatus | null>;
  isInstalled: (name: string) => boolean;
  onToolOpen: (id: string, toolName: string) => void;
  onEdit: (project: Project) => void;
  onRemove: (id: string) => void;
  onToggleStar: (id: string) => void;
  emptyText?: string;
  sessions?: Session[];
  agents?: AgentInfo[];
  requirements?: Requirement[];
  onOpenAgent?: (project: Project) => void;
}

export function ProjectGrid({ projects, gitStatusMap, isInstalled, onToolOpen, onEdit, onRemove, onToggleStar, emptyText, sessions = [], agents = [], requirements = [], onOpenAgent }: ProjectGridProps) {
  if (projects.length === 0) {
    return (
      <div className="project-grid-empty">
        <IconInbox className="project-grid-empty-icon" />
        <p>{emptyText || '暂无项目，点击右下角按钮添加'}</p>
      </div>
    );
  }

  return (
    <div className="project-grid">
      {projects.map(project => (
        <ProjectCard
          key={project.id}
          project={project}
          gitStatus={gitStatusMap[project.path] ?? null}
          isInstalled={isInstalled}
          onToolOpen={onToolOpen}
          onEdit={onEdit}
          onRemove={onRemove}
          onToggleStar={onToggleStar}
          sessions={sessions.filter(s => s.projectPath === project.path)}
          agents={agents}
          requirements={requirements.filter(r => r.projectPath === project.path)}
          onOpenAgent={onOpenAgent}
        />
      ))}
    </div>
  );
}
