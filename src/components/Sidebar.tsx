import { useState, useMemo, useEffect, useRef } from 'react';
import type { Session, Requirement, Project } from '../types';
import { IconStop, IconPlus, IconFolderOpen, IconSearch, IconChat, IconOrchestrate, IconSkillMarket, IconDashboard, IconUser, IconSettings } from './Icons';
import type { IconProps } from './Icons';
import type { ViewId } from '../stores/navigationStore';
import { useAgentStore } from '../stores/agentStore';
import { useNavigationStore } from '../stores/navigationStore';
import { useProjectStore } from '../stores/projectStore';

/** Merged conversation item — requirement or orphan session */
interface ConversationItem {
  id: string;
  title: string;
  status: string;
  sessionId: string | null;
  isRequirement: boolean;
  updatedAt: string;
}

function formatElapsed(startedAt: string): string {
  const sec = Math.floor((Date.now() - new Date(startedAt).getTime()) / 1000);
  const m = Math.floor(sec / 60);
  const s = sec % 60;
  return m > 0 ? `${m}:${String(s).padStart(2, '0')}` : `${s}s`;
}

const STATUS_ICON: Record<string, string> = {
  todo: '☐',
  in_progress: '■',
  done: '☑',
  completed: '●',
  failed: '✕',
};

const STATUS_ORDER: Record<string, number> = {
  in_progress: 0,
  completed: 1,
  failed: 2,
  todo: 3,
  done: 4,
};

/** Views shown in the left column — settings lives in the footer (zcode alignment) */
const VIEWS: { id: ViewId; label: string; Icon: React.FC<IconProps> }[] = [
  { id: 'chat', label: '对话', Icon: IconChat },
  { id: 'orchestrate', label: '编排', Icon: IconOrchestrate },
  { id: 'skill-market', label: '技能', Icon: IconSkillMarket },
  { id: 'dashboard', label: '仪表盘', Icon: IconDashboard },
];

export function Sidebar() {
  // All data from stores
  const projects = useProjectStore((s) => s.projects);
  const activeProject = useNavigationStore((s) => s.activeProject);
  const expandedProjectId = useNavigationStore((s) => s.expandedProjectId);
  const selectProject = useNavigationStore((s) => s.selectProject);
  const selectSession = useNavigationStore((s) => s.selectSession);
  const toggleProjectExpand = useNavigationStore((s) => s.toggleProjectExpand);
  const sessions = useAgentStore((s) => s.sessions);
  const requirements = useAgentStore((s) => s.requirements);
  const activeSessionId = useNavigationStore((s) => s.selectedSessionId);
  const stopAgent = useAgentStore((s) => s.stopAgent);
  const deleteRequirement = useAgentStore((s) => s.removeRequirement);
  const newConversation = useAgentStore((s) => s.newConversation);
  const getDefaultAgent = useAgentStore((s) => s.getDefaultAgent);
  const toggleCommandPalette = useNavigationStore((s) => s.toggleCommandPalette);
  const setAddProjectOpen = useNavigationStore((s) => s.setAddProjectOpen);
  const activeView = useNavigationStore((s) => s.activeView);
  const setActiveView = useNavigationStore((s) => s.setActiveView);
  const sidebarOpen = useNavigationStore((s) => s.sidebarOpen);
  const toggleSidebar = useNavigationStore((s) => s.toggleSidebar);

  const handleToggleProject = (project: typeof projects[0]) => {
    toggleProjectExpand(project.id);
  };

  const handleSelectProject = (project: typeof projects[0]) => {
    if (activeProject?.id === project.id) {
      toggleProjectExpand(project.id);
      return;
    }
    selectProject(project);
    selectSession(null);
    toggleProjectExpand(project.id);
  };

  const handleNewConversation = () => {
    const agent = getDefaultAgent();
    if (agent && activeProject) {
      newConversation(activeProject.path, '新对话', agent);
    }
  };

  const handleSidebarNewConversation = (projectPath: string, title: string) => {
    const agent = getDefaultAgent();
    if (agent) newConversation(projectPath, title, agent);
  };

  const handleSelectSession = (sessionId: string) => {
    selectSession(sessionId);
  };

  const handleSelectRequirement = (_reqId: string) => {
    selectSession(null);
  };

  return (
    <aside className="left-column">
      {/* Header — logo + collapse toggle (zcode-style single column) */}
      <div className="left-column-header">
        <div className="left-column-logo" title="Dev Workbench">DW</div>
        <button
          className="left-column-toggle"
          onClick={toggleSidebar}
          title={sidebarOpen ? '收起边栏' : '展开边栏'}
          aria-label="切换边栏"
        >
          <svg width="16" height="16" viewBox="0 0 16 16" fill="none" aria-hidden="true">
            <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" stroke="currentColor" strokeWidth="1.3" />
            <line x1="5.5" y1="2.5" x2="5.5" y2="13.5" stroke="currentColor" strokeWidth="1.3" />
          </svg>
        </button>
      </div>

      {/* View switcher — settings lives in the footer (zcode alignment) */}
      <nav className="left-column-views" aria-label="Views">
        {VIEWS.map((view) => (
          <button
            key={view.id}
            className={`left-column-view-item ${activeView === view.id ? 'active' : ''}`}
            onClick={() => setActiveView(view.id)}
            title={view.label}
            aria-selected={activeView === view.id}
          >
            <view.Icon size={15} className="left-column-view-icon" />
            <span className="left-column-view-label">{view.label}</span>
          </button>
        ))}
      </nav>

      {/* Quick actions — zcode-style vertical menu (icon + label + shortcut) */}
      <div className="sidebar-quick-actions">
        <button className="sidebar-quick-btn" onClick={handleNewConversation} title="新建对话 (Ctrl+N)">
          <IconPlus size={15} className="sidebar-quick-icon" />
          <span className="sidebar-quick-label">新建</span>
          <span className="sidebar-quick-shortcut">Ctrl+N</span>
        </button>
        <button className="sidebar-quick-btn" onClick={toggleCommandPalette} title="搜索 (Ctrl+K)">
          <IconSearch size={15} className="sidebar-quick-icon" />
          <span className="sidebar-quick-label">搜索</span>
          <span className="sidebar-quick-shortcut">Ctrl+K</span>
        </button>
        <button className="sidebar-quick-btn" onClick={() => setAddProjectOpen(true)} title="打开项目">
          <IconFolderOpen size={15} className="sidebar-quick-icon" />
          <span className="sidebar-quick-label">打开项目</span>
        </button>
      </div>

      {/* Project tree */}
      <div className="sidebar-scroll">
        {projects.map(project => (
          <ProjectGroup
            key={project.id}
            project={project}
            isExpanded={expandedProjectId === project.id}
            isActive={activeProject?.id === project.id}
            onToggle={() => handleToggleProject(project)}
            onSelect={() => handleSelectProject(project)}
            sessions={sessions}
            requirements={requirements}
            activeSessionId={activeSessionId}
            onSelectSession={handleSelectSession}
            onSelectRequirement={handleSelectRequirement}
            onNewConversation={handleSidebarNewConversation}
            onDeleteRequirement={deleteRequirement}
            onStopSession={stopAgent}
          />
        ))}
      </div>

      {/* Footer — user + settings (zcode alignment: settings lives here, not in view switcher) */}
      <div className="left-column-footer">
        <button className="left-column-user" title="用户">
          <IconUser size={16} />
          <span className="left-column-user-label">用户</span>
        </button>
        <button
          className={`left-column-settings ${activeView === 'settings' ? 'active' : ''}`}
          onClick={() => setActiveView('settings')}
          title="设置"
          aria-selected={activeView === 'settings'}
        >
          <IconSettings size={16} />
        </button>
      </div>
    </aside>
  );
}

// ─── Project Group (expandable) ───

function ProjectGroup({
  project,
  isExpanded,
  isActive,
  onToggle,
  onSelect,
  sessions,
  requirements,
  activeSessionId,
  onSelectSession,
  onSelectRequirement,
  onNewConversation,
  onDeleteRequirement,
  onStopSession,
}: {
  project: Project;
  isExpanded: boolean;
  isActive: boolean;
  onToggle: () => void;
  onSelect: () => void;
  sessions: Session[];
  requirements: Requirement[];
  activeSessionId: string | null;
  onSelectSession: (id: string) => void;
  onSelectRequirement: (id: string) => void;
  onNewConversation: (path: string, title: string) => void;
  onDeleteRequirement: (id: string) => void;
  onStopSession: (id: string) => void;
}) {
  const [showInput, setShowInput] = useState(false);
  const [inputTitle, setInputTitle] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  const runningCount = sessions.filter(
    s => s.projectPath === project.path && s.status === 'running'
  ).length;

  // Build merged conversation list
  const items = useMemo<ConversationItem[]>(() => {
    const reqItems: ConversationItem[] = requirements
      .filter(r => r.projectPath === project.path)
      .map(r => ({
        id: r.id,
        title: r.title,
        status: r.status,
        sessionId: r.linkedSessionId,
        isRequirement: true,
        updatedAt: r.updatedAt,
      }));

    const linkedReqIds = new Set(
      requirements
        .filter(r => r.projectPath === project.path && r.linkedSessionId)
        .map(r => r.linkedSessionId)
    );

    const orphanItems: ConversationItem[] = sessions
      .filter(s => s.projectPath === project.path && !linkedReqIds.has(s.id))
      .map(s => ({
        id: s.id,
        title: s.prompt.slice(0, 60),
        status: s.status,
        sessionId: s.id,
        isRequirement: false,
        updatedAt: s.finishedAt || s.startedAt,
      }));

    return [...reqItems, ...orphanItems].sort(
      (a, b) => (STATUS_ORDER[a.status] ?? 5) - (STATUS_ORDER[b.status] ?? 5)
    );
  }, [requirements, sessions, project.path]);

  // Check if any item is running
  const runningSession = useMemo(
    () => sessions.find(s => s.projectPath === project.path && s.status === 'running'),
    [sessions, project.path]
  );

  // Determine which item is selected
  const selectedItem = useMemo(() => {
    if (!activeSessionId) return null;
    return items.find(i => i.sessionId === activeSessionId || i.id === activeSessionId) ?? null;
  }, [items, activeSessionId]);

  const handleSubmitNew = () => {
    const t = inputTitle.trim();
    if (t) {
      onNewConversation(project.path, t);
      setInputTitle('');
      setShowInput(false);
    }
  };

  useEffect(() => {
    if (showInput) inputRef.current?.focus();
  }, [showInput]);

  const handleAddClick = () => {
    if (!isExpanded) onToggle();
    setShowInput(true);
  };

  return (
    <div className={`sidebar-project-group ${isActive ? 'active' : ''}`}>
      <div className="sidebar-project-header">
        <button
          className="sidebar-project-expand-btn"
          onClick={onToggle}
          title={isExpanded ? '折叠' : '展开'}
        >
          <span className="sidebar-project-expand">{isExpanded ? '▾' : '▸'}</span>
        </button>
        <button
          className="sidebar-project-name-btn"
          onClick={onSelect}
          title={project.path}
        >
          <span className="sidebar-project-name">{project.name}</span>
          {runningCount > 0 && (
            <span className="sidebar-project-badge">{runningCount}</span>
          )}
        </button>
        <button className="sidebar-project-add" onClick={handleAddClick} title="新建对话">
          +
        </button>
      </div>

      {isExpanded && (
        <div className="sidebar-conversations">
          {items.length === 0 && !showInput && (
            <div className="sidebar-conversations-empty">暂无对话</div>
          )}
          {items.map(item => (
            <ConversationRow
              key={item.id}
              item={item}
              isSelected={selectedItem?.id === item.id}
              runningSession={item.sessionId && runningSession?.id === item.sessionId ? runningSession : null}
              onSelect={() => {
                if (item.sessionId) {
                  onSelectSession(item.sessionId);
                } else if (item.isRequirement && item.status === 'todo') {
                  onSelectRequirement(item.id);
                }
              }}
              onDelete={item.isRequirement && item.status === 'todo' ? () => onDeleteRequirement(item.id) : undefined}
              onStop={onStopSession}
            />
          ))}
          {showInput && (
            <div className="sidebar-conversation-input-row">
              <input
                ref={inputRef}
                className="sidebar-conversation-input"
                type="text"
                placeholder="对话标题..."
                value={inputTitle}
                onChange={e => setInputTitle(e.target.value)}
                onKeyDown={e => {
                  if (e.key === 'Enter') handleSubmitNew();
                  if (e.key === 'Escape') { setShowInput(false); setInputTitle(''); }
                }}
                maxLength={200}
              />
              <button className="sidebar-conversation-input-confirm" onClick={handleSubmitNew} disabled={!inputTitle.trim()}>
                ✓
              </button>
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ─── Conversation Row ───

function ConversationRow({
  item,
  isSelected,
  runningSession,
  onSelect,
  onDelete,
  onStop,
}: {
  item: ConversationItem;
  isSelected: boolean;
  runningSession: Session | null;
  onSelect: () => void;
  onDelete?: () => void;
  onStop: (id: string) => void;
}) {
  const icon = STATUS_ICON[item.status] || '○';
  const isRunning = !!runningSession;

  return (
    <div className={`sidebar-conversation-item ${item.status} ${isSelected ? 'selected' : ''}`}>
      <button className="sidebar-conversation-btn" onClick={onSelect} title={item.title}>
        <span className="sidebar-conversation-icon">{icon}</span>
        <span className="sidebar-conversation-title">{item.title}</span>
        {isRunning && <RunningTimer startedAt={runningSession.startedAt} />}
      </button>
      {isRunning ? (
        <button className="sidebar-conversation-stop" onClick={() => onStop(runningSession.id)} title="停止">
          <IconStop size={10} />
        </button>
      ) : onDelete ? (
        <button className="sidebar-conversation-delete" onClick={onDelete} title="删除">
          ×
        </button>
      ) : null}
    </div>
  );
}

function RunningTimer({ startedAt }: { startedAt: string }) {
  const [elapsed, setElapsed] = useState(() => formatElapsed(startedAt));
  useEffect(() => {
    const id = setInterval(() => setElapsed(formatElapsed(startedAt)), 1000);
    return () => clearInterval(id);
  }, [startedAt]);
  return <span className="sidebar-conversation-elapsed">{elapsed}</span>;
}
