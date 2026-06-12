import type { Project, GitStatus, Session, AgentInfo, Requirement } from '../types';
import { ToolButton } from './ToolButton';
import { AgentStatus } from './AgentStatus';
import { IconStar, IconEdit, IconTrash, IconSparkles } from './Icons';
import { formatRelativeTime } from '../utils/formatRelativeTime';

// Non-agent tools that are always shown on cards (IDE, file explorer)
const NON_AGENT_TOOLS = [
  { name: 'code', label: 'VSCode' },
  { name: 'finder', label: 'Files' },
];

interface ProjectCardProps {
  project: Project;
  gitStatus: GitStatus | null;
  isInstalled: (name: string) => boolean;
  onToolOpen: (id: string, toolName: string) => void;
  onEdit: (project: Project) => void;
  onRemove: (id: string) => void;
  onToggleStar: (id: string) => void;
  sessions?: Session[];
  agents?: AgentInfo[];
  requirements?: Requirement[];
  onOpenAgent?: (project: Project) => void;
}

export function ProjectCard({ project, gitStatus, isInstalled, onToolOpen, onEdit, onRemove, onToggleStar, sessions = [], agents = [], requirements = [], onOpenAgent }: ProjectCardProps) {
  const lastOpened = project.last_opened_at
    ? new Date(project.last_opened_at).toLocaleString('zh-CN')
    : '尚未打开';

  // Merge agent tools (from discovery) + non-agent tools (IDE, Files)
  const agentTools = agents
    .filter(a => a.installed)
    .map(a => ({
      name: a.commandName,
      label: a.displayName,
      installed: true,
    }));

  const nonAgentTools = NON_AGENT_TOOLS.map(t => ({
    name: t.name,
    label: t.label,
    installed: t.name === 'finder' ? true : isInstalled(t.name),
  }));

  const displayTools = [...agentTools, ...nonAgentTools];

  return (
    <div className="project-card">
      <div className="card-body">
        <div style={{ display: 'flex', alignItems: 'flex-start', justifyContent: 'space-between' }}>
          <h3 className="card-title">{project.name}</h3>
          <button
            className={`star-btn`}
            style={{ position: 'static', flexShrink: 0 }}
            onClick={() => onToggleStar(project.id)}
            title={project.starred ? '取消收藏' : '收藏'}
          >
            <IconStar size={14} filled={project.starred} />
          </button>
        </div>
        {project.description && <p className="card-desc">{project.description}</p>}

        <div className="card-meta">
          {gitStatus?.lastCommitTime ? (
            <span className="card-time" title={new Date(gitStatus.lastCommitTime).toLocaleString('zh-CN')}>
              last commit {formatRelativeTime(gitStatus.lastCommitTime)}
            </span>
          ) : (
            <span className="card-time">{lastOpened}</span>
          )}
          {project.open_count > 0 && (
            <span className="card-count">打开 {project.open_count} 次</span>
          )}
        </div>

        {/* Git status row */}
        {gitStatus && (
          <div className="card-meta" style={{ marginTop: 2 }}>
            <span className="git-branch" style={{ fontSize: 11, maxWidth: 200 }}>
              {gitStatus.branch}
            </span>
            {gitStatus.isDirty && <span className="git-dirty" title="有未提交变更" />}
            {gitStatus.ahead > 0 && (
              <span className="git-ahead" title={`${gitStatus.ahead} 个 commit 未推送`}>↑{gitStatus.ahead}</span>
            )}
            {gitStatus.behind > 0 && (
              <span className="git-behind" title={`${gitStatus.behind} 个 commit 未拉取`}>↓{gitStatus.behind}</span>
            )}
          </div>
        )}

        {project.tags.length > 0 && (
          <div className="card-tags">
            {project.tags.map(tag => (
              <span key={tag} className="tag">{tag}</span>
            ))}
            {requirements.length > 0 && (
              <span className="agent-badge">{requirements.length} 需求</span>
            )}
          </div>
        )}

        <div className="card-path" title={project.path}>
          {project.path}
        </div>
      </div>

      <div className="card-tools">
        {displayTools.map(tool => (
          <ToolButton
            key={tool.name}
            tool={tool.name}
            projectPath={project.path}
            installed={tool.installed}
            label={tool.label}
            onClick={() => onToolOpen(project.id, tool.name)}
          />
        ))}
      </div>

      <AgentStatus sessions={sessions} agents={agents} />

      <div className="card-actions">
        {onOpenAgent && (
          <button className="action-btn agent-action" onClick={() => onOpenAgent(project)} title="Agent Hub">
            <IconSparkles size={15} />
          </button>
        )}
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
