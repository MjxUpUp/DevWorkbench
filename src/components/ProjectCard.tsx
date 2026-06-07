import type { Project, GitStatus } from '../types';
import { ToolButton, TOOL_LABELS } from './ToolButton';
import { IconStar, IconEdit, IconTrash, IconSparkles, IconTerminal } from './Icons';
import { useToast } from './Toast';
import { launchTool } from '../utils/launchTool';

const ALL_TOOLS = ['claude', 'cursor', 'code', 'finder'] as const;

interface ProjectCardProps {
  project: Project;
  gitStatus: GitStatus | null;
  isInstalled: (name: string) => boolean;
  onToolOpen: (id: string, toolName: string) => void;
  onEdit: (project: Project) => void;
  onRemove: (id: string) => void;
  onToggleStar: (id: string) => void;
}

function formatRelativeTime(isoTime: string): string {
  const now = Date.now();
  const then = new Date(isoTime).getTime();
  const diffSec = Math.floor((now - then) / 1000);

  if (diffSec < 60) return '刚刚';
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)} 分钟前`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)} 小时前`;
  if (diffSec < 2592000) return `${Math.floor(diffSec / 86400)} 天前`;
  if (diffSec < 31536000) return `${Math.floor(diffSec / 2592000)} 个月前`;
  return `${Math.floor(diffSec / 31536000)} 年前`;
}

export function ProjectCard({ project, gitStatus, isInstalled, onToolOpen, onEdit, onRemove, onToggleStar }: ProjectCardProps) {
  const toast = useToast();
  const lastOpened = project.last_opened_at
    ? new Date(project.last_opened_at).toLocaleString('zh-CN')
    : '尚未打开';

  // 用户配置的工具组合（优先），否则全量显示
  const displayTools = project.workspace_tools.length > 0
    ? project.workspace_tools
    : ALL_TOOLS;

  const canRestore = project.last_opened_tools.length >= 2;
  const canLaunchAll = project.workspace_tools.length >= 2;

  const handleRestoreWorkspace = async () => {
    let successCount = 0;
    let lastError = '';
    for (const tool of project.last_opened_tools) {
      try {
        await launchTool(tool, project.path);
        successCount++;
      } catch (e) {
        lastError = String(e);
      }
    }
    if (successCount > 0) {
      toast.info(`已恢复工作区，启动 ${successCount} 个工具`);
    } else if (lastError) {
      toast.error(`恢复工作区失败: ${lastError}`);
    }
  };

  const handleLaunchAll = async () => {
    let successCount = 0;
    let lastError = '';
    for (const tool of project.workspace_tools) {
      try {
        await launchTool(tool, project.path);
        successCount++;
      } catch (e) {
        lastError = String(e);
      }
    }
    if (successCount > 0) {
      toast.info(`一键启动完成，启动 ${successCount} 个工具`);
    } else if (lastError) {
      toast.error(`一键启动失败: ${lastError}`);
    }
  };

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
        {/* Git 状态 badge 覆盖在封面左下角 */}
        {gitStatus && (
          <div className="git-badge">
            <span className="git-branch">{gitStatus.branch}</span>
            {gitStatus.isDirty && <span className="git-dirty" title="有未提交变更" />}
            {gitStatus.ahead > 0 && (
              <span className="git-ahead" title={`${gitStatus.ahead} 个 commit 未推送`}>↑{gitStatus.ahead}</span>
            )}
            {gitStatus.behind > 0 && (
              <span className="git-behind" title={`${gitStatus.behind} 个 commit 未拉取`}>↓{gitStatus.behind}</span>
            )}
          </div>
        )}
      </div>

      <div className="card-body">
        <h3 className="card-title">{project.name}</h3>
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
        {displayTools.map(tool => (
          <ToolButton
            key={tool}
            tool={tool}
            projectPath={project.path}
            installed={tool === 'finder' ? true : isInstalled(tool)}
            onClick={() => onToolOpen(project.id, tool)}
          />
        ))}
        {canLaunchAll && (
          <button
            className="launch-all-btn"
            onClick={handleLaunchAll}
            title={`一键启动: ${project.workspace_tools.map(t => TOOL_LABELS[t] || t).join(' + ')}`}
          >
            <span className="tool-btn-icon"><IconTerminal size={14} /></span>
            <span className="tool-btn-label">全部</span>
          </button>
        )}
        {!canLaunchAll && canRestore && (
          <button
            className="workspace-restore-btn"
            onClick={handleRestoreWorkspace}
            title={`恢复工作区: ${project.last_opened_tools.map(t => TOOL_LABELS[t] || t).join(' + ')}`}
          >
            <span className="tool-btn-icon"><IconSparkles size={14} /></span>
            <span className="tool-btn-label">恢复</span>
          </button>
        )}
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
