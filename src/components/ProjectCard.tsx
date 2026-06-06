import type { Project } from '../types';
import { ToolButton } from './ToolButton';
import { IconStar, IconEdit, IconTrash } from './Icons';

interface ProjectCardProps {
  project: Project;
  isInstalled: (name: string) => boolean;
  onOpen: (id: string) => void;
  onEdit: (project: Project) => void;
  onRemove: (id: string) => void;
  onToggleStar: (id: string) => void;
}

export function ProjectCard({ project, isInstalled, onOpen, onEdit, onRemove, onToggleStar }: ProjectCardProps) {
  const lastOpened = project.last_opened_at
    ? new Date(project.last_opened_at).toLocaleString('zh-CN')
    : '尚未打开';

  return (
    <div className="project-card">
      <div className="card-cover">
        {project.cover_image ? (
          <img src={project.cover_image} alt={project.name} />
        ) : (
          <div className="card-cover-placeholder">
            <span className="cover-text">{project.name.slice(0, 2).toUpperCase()}</span>
          </div>
        )}
        <button
          className={`star-btn ${project.starred ? 'starred' : ''}`}
          onClick={() => onToggleStar(project.id)}
          title={project.starred ? '取消收藏' : '收藏'}
        >
          <IconStar size={14} filled={project.starred} />
        </button>
      </div>

      <div className="card-body">
        <h3 className="card-title">{project.name}</h3>
        {project.description && <p className="card-desc">{project.description}</p>}

        <div className="card-meta">
          <span className="card-time">{lastOpened}</span>
          {project.open_count > 0 && (
            <span className="card-count">打开 {project.open_count} 次</span>
          )}
        </div>

        {project.tags.length > 0 && (
          <div className="card-tags">
            {project.tags.map(tag => (
              <span key={tag} className="tag">{tag}</span>
            ))}
          </div>
        )}

        <div className="card-path" title={project.path}>
          {project.path}
        </div>
      </div>

      <div className="card-tools">
        <ToolButton tool="claude" projectPath={project.path} installed={isInstalled('claude')} onClick={() => onOpen(project.id)} />
        <ToolButton tool="cursor" projectPath={project.path} installed={isInstalled('cursor')} onClick={() => onOpen(project.id)} />
        <ToolButton tool="code" projectPath={project.path} installed={isInstalled('code')} onClick={() => onOpen(project.id)} />
        <ToolButton tool="terminal" projectPath={project.path} installed={true} onClick={() => onOpen(project.id)} />
        <ToolButton tool="finder" projectPath={project.path} installed={true} onClick={() => onOpen(project.id)} />
      </div>

      <div className="card-actions">
        <button className="action-btn" onClick={() => onEdit(project)} title="编辑">
          <IconEdit size={15} />
        </button>
        <button className="action-btn danger" onClick={() => onRemove(project.id)} title="删除">
          <IconTrash size={15} />
        </button>
      </div>
    </div>
  );
}
