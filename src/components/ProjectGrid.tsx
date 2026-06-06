import type { Project } from '../types';
import { ProjectCard } from './ProjectCard';

interface ProjectGridProps {
  projects: Project[];
  isInstalled: (name: string) => boolean;
  onOpen: (id: string) => void;
  onEdit: (project: Project) => void;
  onRemove: (id: string) => void;
  onToggleStar: (id: string) => void;
  emptyText?: string;
}

export function ProjectGrid({ projects, isInstalled, onOpen, onEdit, onRemove, onToggleStar, emptyText }: ProjectGridProps) {
  if (projects.length === 0) {
    return (
      <div className="project-grid-empty">
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
          isInstalled={isInstalled}
          onOpen={onOpen}
          onEdit={onEdit}
          onRemove={onRemove}
          onToggleStar={onToggleStar}
        />
      ))}
    </div>
  );
}
