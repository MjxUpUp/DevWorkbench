import { useState, useRef, useEffect } from 'react';
import type { Project } from '../types';
import { IconSearch, IconSparkles, IconPlus, IconUser, IconSettings, IconOrchestrate } from './Icons';
import type { IconProps } from './Icons';
import type { ViewId } from '../stores/navigationStore';
import { useNavigationStore } from '../stores/navigationStore';
import { useProjectStore } from '../stores/projectStore';

/**
 * Primary navigation — aligns to the target layout:
 *   logo (Z, toggles the column) → 创建任务 / 搜索 / 技能 → 工作区 (flat project
 *   list, no conversation tree) → 用户资料 (opens a menu with 设置).
 *
 * The conversation tree that used to live here is removed: sessions are now
 * managed in the main task view. "新建" (Ctrl+N) lives in the task empty-state
 * and the user menu.
 */
const VIEWS: { id: ViewId; label: string; Icon: React.FC<IconProps> }[] = [
  { id: 'task', label: '创建任务', Icon: IconPlus },
  { id: 'search', label: '搜索', Icon: IconSearch },
  { id: 'skills', label: '技能', Icon: IconSparkles },
  { id: 'orchestrate', label: '编排', Icon: IconOrchestrate },
];

export function Sidebar() {
  const projects = useProjectStore((s) => s.projects);
  const activeProject = useNavigationStore((s) => s.activeProject);
  const selectProject = useNavigationStore((s) => s.selectProject);
  const selectSession = useNavigationStore((s) => s.selectSession);
  const toggleSidebar = useNavigationStore((s) => s.toggleSidebar);
  const sidebarOpen = useNavigationStore((s) => s.sidebarOpen);
  const activeView = useNavigationStore((s) => s.activeView);
  const setActiveView = useNavigationStore((s) => s.setActiveView);

  const handleSelectProject = (project: Project) => {
    if (activeProject?.id === project.id) return;
    selectProject(project);
    selectSession(null);
    // Picking a project implies working on a task.
    if (activeView !== 'task') setActiveView('task');
  };

  return (
    <aside className="left-column">
      {/* Logo zone — doubles as the sidebar toggle (zcode convention) */}
      <button
        className="left-column-logo"
        onClick={toggleSidebar}
        title={sidebarOpen ? '收起边栏' : '展开边栏'}
        aria-label="切换边栏"
        aria-expanded={sidebarOpen}
        type="button"
      >
        Z
      </button>

      {/* Primary navigation — 创建任务 / 搜索 / 技能 */}
      <nav className="left-column-nav" aria-label="主导航">
        {VIEWS.map((view) => (
          <button
            key={view.id}
            className={`left-column-nav-item ${activeView === view.id ? 'active' : ''}`}
            onClick={() => setActiveView(view.id)}
            title={view.label}
            aria-selected={activeView === view.id}
          >
            <view.Icon size={16} className="left-column-nav-icon" />
            <span className="left-column-nav-label">{view.label}</span>
          </button>
        ))}
      </nav>

      {/* Workspace — flat project list (conversations live in the main view now) */}
      <div className="left-column-section-header">
        <span>工作区</span>
      </div>
      <div className="left-column-projects">
        {projects.length === 0 && (
          <div className="left-column-projects-empty">暂无项目</div>
        )}
        {projects.map((project) => (
          <button
            key={project.id}
            className={`left-column-project ${activeProject?.id === project.id ? 'active' : ''}`}
            onClick={() => handleSelectProject(project)}
            title={project.path}
          >
            <span className="left-column-project-name">{project.name}</span>
          </button>
        ))}
      </div>

      {/* Footer — user profile with a settings menu */}
      <UserMenu />
    </aside>
  );
}

/**
 * User profile block. Shows the configured display name (placeholder account)
 * and opens a small menu with 设置 / 新建对话.
 */
function UserMenu() {
  const setActiveView = useNavigationStore((s) => s.setActiveView);
  const setAddProjectOpen = useNavigationStore((s) => s.setAddProjectOpen);
  const [open, setOpen] = useState(false);
  const wrapRef = useRef<HTMLDivElement>(null);

  // Display name — local placeholder until a real account system exists.
  const displayName = '旅行者5655';

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (wrapRef.current && !wrapRef.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  return (
    <div className="left-column-user-wrap" ref={wrapRef}>
      <button
        className="left-column-user"
        onClick={() => setOpen((v) => !v)}
        title="账户"
        aria-haspopup="menu"
        aria-expanded={open}
        type="button"
      >
        <span className="left-column-user-avatar"><IconUser size={16} /></span>
        <span className="left-column-user-name">{displayName}</span>
      </button>
      {open && (
        <div className="left-column-user-menu" role="menu">
          <button
            className="left-column-user-menu-item"
            role="menuitem"
            onClick={() => { setActiveView('settings'); setOpen(false); }}
          >
            <IconSettings size={14} /> 设置
          </button>
          <button
            className="left-column-user-menu-item"
            role="menuitem"
            onClick={() => { setAddProjectOpen(true); setOpen(false); }}
          >
            <IconPlus size={14} /> 添加项目
          </button>
        </div>
      )}
    </div>
  );
}
