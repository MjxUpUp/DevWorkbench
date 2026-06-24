import { useState, useRef, useEffect, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { Project } from '../types';
import { IconSearch, IconSparkles, IconPlus, IconUser, IconSettings, IconOrchestrate, IconTrash } from './Icons';
import type { IconProps } from './Icons';
import type { ViewId } from '../stores/navigationStore';
import { useNavigationStore } from '../stores/navigationStore';
import { useProjectStore } from '../stores/projectStore';
import { useAgentStore } from '../stores/agentStore';
import { useToast } from './Toast';

/**
 * Primary navigation — aligns to the target layout:
 *   创建任务 / 搜索 / 技能 → 工作区 (project list; the active project expands to
 *   show its conversations — the topic containers) → 用户资料 (设置 menu).
 *
 * A conversation is the Claude-Code "session": a multi-turn topic. Selecting one
 * loads its turns in the main task view.
 */
const VIEWS: { id: ViewId; label: string; Icon: React.FC<IconProps> }[] = [
  { id: 'task', label: '创建任务', Icon: IconPlus },
  { id: 'search', label: '搜索', Icon: IconSearch },
  { id: 'skills', label: '技能', Icon: IconSparkles },
  { id: 'orchestrate', label: '编排', Icon: IconOrchestrate },
];

export function Sidebar() {
  const projects = useProjectStore((s) => s.projects);
  const removeProject = useProjectStore((s) => s.removeProject);
  const activeProject = useNavigationStore((s) => s.activeProject);
  const selectProject = useNavigationStore((s) => s.selectProject);
  const selectConversation = useNavigationStore((s) => s.selectConversation);
  const selectedConversationId = useNavigationStore((s) => s.selectedConversationId);
  const activeView = useNavigationStore((s) => s.activeView);
  const setActiveView = useNavigationStore((s) => s.setActiveView);
  const setCommandPaletteOpen = useNavigationStore((s) => s.setCommandPaletteOpen);

  const handleNavClick = (view: { id: ViewId; label: string }) => {
    if (view.id === 'search') {
      setCommandPaletteOpen(true);
      return;
    }
    // "创建任务"：如果已在 task 视图且有选中对话，清空对话以展示空态
    if (view.id === 'task' && activeView === 'task' && selectedConversationId) {
      selectConversation(null);
      return;
    }
    setActiveView(view.id);
  };

  const handleSelectProject = (project: Project) => {
    if (activeProject?.id === project.id) return;
    selectProject(project);
    // selectProject already clears selectedConversationId.
    if (activeView !== 'task') setActiveView('task');
  };

  const handleRemoveProject = (project: Project) => {
    removeProject(project.id).catch((err) => console.error('removeProject failed', err));
  };

  return (
    <aside className="left-column">
      {/* Primary navigation — 创建任务 / 搜索 / 技能. The brand mark and the
          sidebar toggle both live in the title bar (TitleBar.tsx); not
          duplicated as a "Z" logo here. */}
      <nav className="left-column-nav" aria-label="主导航">
        {VIEWS.map((view) => (
          <button
            key={view.id}
            className={`left-column-nav-item ${activeView === view.id ? 'active' : ''}`}
            // "搜索" opens the command palette as a transparent centered modal
            // that queries conversation history, instead of switching views.
            onClick={() => handleNavClick(view)}
            title={view.label}
            aria-selected={activeView === view.id}
          >
            <view.Icon size={16} className="left-column-nav-icon" />
            <span className="left-column-nav-label">{view.label}</span>
          </button>
        ))}
      </nav>

      {/* Workspace — project list; the active project shows its conversations. */}
      <div className="left-column-section-header">
        <span>工作区</span>
      </div>
      <div className="left-column-projects">
        {projects.length === 0 && (
          <div className="left-column-projects-empty">暂无项目</div>
        )}
        {projects.map((project) => (
          <div key={project.id}>
            <div
              className={`left-column-project ${activeProject?.id === project.id ? 'active' : ''}`}
              onClick={() => handleSelectProject(project)}
              title={project.path}
              role="button"
              tabIndex={0}
              onKeyDown={(e) => {
                if (e.key === 'Enter' || e.key === ' ') {
                  e.preventDefault();
                  handleSelectProject(project);
                }
              }}
            >
              <span className="left-column-project-name" data-testid="left-column-project">{project.name}</span>
              <button
                className="left-column-project-remove"
                onClick={(e) => { e.stopPropagation(); handleRemoveProject(project); }}
                title="移除项目"
                aria-label={`移除 ${project.name}`}
                type="button"
              >
                <IconTrash size={14} />
              </button>
            </div>
            {/* Expand the active project's conversations inline. This is the
                topic list — selecting one loads its turns in the main view. */}
            {activeProject?.id === project.id && (
              <ConversationList
                projectPath={project.path}
                selectedId={selectedConversationId}
                onSelect={selectConversation}
              />
            )}
          </div>
        ))}
      </div>

      {/* Footer — user profile with a settings menu */}
      <UserMenu />
    </aside>
  );
}

/**
 * The conversation (topic) list under the active project. Newest activity first;
 * pinned float to the top — both enforced by getConversationsForProject.
 */
function ConversationList({
  projectPath,
  selectedId,
  onSelect,
}: {
  projectPath: string;
  selectedId: string | null;
  onSelect: (id: string) => void;
}) {
  // Subscribe to the raw conversations array so the list re-renders when
  // refreshConversations populates it. Calling getConversationsForProject as a
  // method selector returns a stable function reference and does NOT subscribe
  // to the underlying data — so an async refresh that fills store.conversations
  // never triggered a re-render, and the list stayed "暂无对话" forever.
  const allConversations = useAgentStore((s) => s.conversations);
  const refreshConversations = useAgentStore((s) => s.refreshConversations);
  const archiveConversation = useAgentStore((s) => s.archiveConversation);
  const deleteConversation = useAgentStore((s) => s.deleteConversation);
  const toast = useToast();

  // A3 archive (soft-hide) + delete (soft-delete with an undo toast that
  // restores the row to 'active'). Both go through store actions that
  // optimistically drop the row from local state before refreshing — without
  // that, refreshConversations' WAL-lag merge re-adds the just-hidden row and
  // the list only updates after an app restart.
  const handleArchive = async (id: string) => {
    try {
      await archiveConversation(id, projectPath);
      toast.info('已归档');
    } catch (e) {
      toast.error(`归档失败: ${e}`);
    }
  };

  const handleDelete = async (id: string) => {
    try {
      await deleteConversation(id, projectPath);
      toast.success('对话已删除', {
        label: '撤销',
        onClick: async () => {
          try {
            await invoke('restore_conversation', { id });
            await refreshConversations(projectPath);
            toast.info('已恢复');
          } catch (err) {
            toast.error(`恢复失败: ${err}`);
          }
        },
      });
    } catch (e) {
      toast.error(`删除失败: ${e}`);
    }
  };

  const conversations = useMemo(
    () =>
      allConversations
        .filter((c) => c.projectPath === projectPath)
        .sort((a, b) => {
          if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
          return new Date(b.lastActivityAt).getTime() - new Date(a.lastActivityAt).getTime();
        }),
    [allConversations, projectPath],
  );

  // Load the conversation list whenever this project becomes active. The store
  // keeps a global pool, but a freshly-switched project may not be populated
  // until this fetch runs.
  //
  // Track which projects have finished their FIRST refresh. Before it resolves,
  // `conversations` is empty — rendering the "暂无对话" empty state there would
  // flash it for a frame before the list pops in (the flicker on first project
  // click). So render nothing during the first load, and only show the empty
  // state once we've confirmed the project genuinely has zero conversations.
  const [loadedPaths, setLoadedPaths] = useState<Set<string>>(new Set());
  useEffect(() => {
    void refreshConversations(projectPath).then(() => {
      setLoadedPaths((prev) =>
        prev.has(projectPath) ? prev : new Set(prev).add(projectPath),
      );
    });
  }, [projectPath, refreshConversations]);

  if (conversations.length === 0) {
    return loadedPaths.has(projectPath) ? (
      <div className="left-column-conversations-empty">暂无对话</div>
    ) : null;
  }

  return (
    <div className="left-column-conversations">
      {conversations.map((c) => (
        <div
          key={c.id}
          className={`left-column-conversation ${selectedId === c.id ? 'active' : ''}`}
          onClick={() => onSelect(c.id)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' || e.key === ' ') {
              e.preventDefault();
              onSelect(c.id);
            }
          }}
          role="button"
          tabIndex={0}
          title={c.title}
        >
          {c.pinned && <span className="left-column-conversation-pin">📌</span>}
          <span className="left-column-conversation-title">{c.title}</span>
          <span
            className="left-column-conversation-actions"
            onClick={(e) => e.stopPropagation()}
          >
            <button
              type="button"
              className="left-column-conversation-action"
              aria-label="归档"
              title="归档"
              onClick={() => handleArchive(c.id)}
            >
              📦
            </button>
            <button
              type="button"
              className="left-column-conversation-action"
              aria-label="删除"
              title="删除"
              onClick={() => handleDelete(c.id)}
            >
              <IconTrash size={13} />
            </button>
          </span>
        </div>
      ))}
    </div>
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
