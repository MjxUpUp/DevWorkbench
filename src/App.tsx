import { useState, useMemo, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Sidebar } from './components/Sidebar';
import { ProjectGrid } from './components/ProjectGrid';
import { AddProject } from './components/AddProject';
import { EditProject } from './components/EditProject';
import { Settings } from './components/Settings';
import { AgentPanel } from './components/AgentPanel';
import { ToastProvider } from './components/Toast';
import { useProjects } from './hooks/useProjects';
import { useTools } from './hooks/useTools';
import { useGitStatus } from './hooks/useGitStatus';
import { useAgents } from './hooks/useAgents';
import { IconSearch, IconPlus, IconFolder, IconClock, IconStar, IconSettings } from './components/Icons';
import type { Project, AppSettings } from './types';
import './styles/index.css';

const SIDEBAR_ITEMS = [
  { key: 'all', label: '全部项目', IconComponent: IconFolder },
  { key: 'recent', label: '最近打开', IconComponent: IconClock },
  { key: 'starred', label: '收藏', IconComponent: IconStar },
];

function App() {
  const [activeView, setActiveView] = useState('all');
  const [showAdd, setShowAdd] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');
  const [editingProject, setEditingProject] = useState<Project | null>(null);
  const [theme, setThemeState] = useState('obsidian');
  const [activeAgentProject, setActiveAgentProject] = useState<Project | null>(null);

  const { projects, addProject, removeProject, updateProject, recordOpen, recordToolOpen, error: projectError } = useProjects();
  const { tools, isInstalled, error: toolsError } = useTools();
  const { gitStatusMap } = useGitStatus(projects);
  const agentHook = useAgents();

  // 启动时加载主题设置
  useEffect(() => {
    invoke<AppSettings>('load_settings')
      .then(s => {
        if (s.theme) {
          setThemeState(s.theme);
        }
      })
      .catch(() => {});
  }, []);

  // 主题变化时应用到 DOM
  useEffect(() => {
    document.documentElement.setAttribute('data-theme', theme);
  }, [theme]);

  const setTheme = async (newTheme: string) => {
    setThemeState(newTheme);
    try {
      const settings = await invoke<AppSettings>('load_settings');
      settings.theme = newTheme;
      await invoke('save_settings', { settings });
    } catch {}
  };

  const filteredProjects = useMemo(() => {
    let list = projects;

    if (activeView === 'recent') {
      list = [...list]
        .filter(p => p.last_opened_at)
        .sort((a, b) => (b.last_opened_at || '').localeCompare(a.last_opened_at || ''));
    } else if (activeView === 'starred') {
      list = list.filter(p => p.starred);
    }

    if (searchQuery.trim()) {
      const q = searchQuery.toLowerCase();
      list = list.filter(p =>
        p.name.toLowerCase().includes(q) ||
        p.description.toLowerCase().includes(q) ||
        p.tags.some(t => t.toLowerCase().includes(q)) ||
        p.path.toLowerCase().includes(q)
      );
    }

    return list;
  }, [projects, activeView, searchQuery]);

  const handleToolOpen = async (id: string, toolName: string) => {
    await recordOpen(id);
    await recordToolOpen(id, toolName);
  };

  const handleToggleStar = async (id: string) => {
    const project = projects.find(p => p.id === id);
    if (project) {
      await updateProject(id, { starred: !project.starred });
    }
  };

  const handleEdit = async (project: Project) => {
    setEditingProject(project);
  };

  const handleRemove = async (id: string) => {
    if (confirm('确定要移除此项目吗？（不会删除本地文件）')) {
      await removeProject(id);
    }
  };

  const emptyTexts: Record<string, string> = {
    all: '暂无项目，点击下方按钮添加你的第一个项目',
    recent: '还没有打开过任何项目',
    starred: '还没有收藏任何项目',
  };

  return (
    <ToastProvider>
    <div className="app">
      <Sidebar
        items={SIDEBAR_ITEMS}
        activeKey={activeView}
        onSelect={setActiveView}
        footer={
          <button className="sidebar-item" onClick={() => setShowSettings(true)}>
            <span className="sidebar-item-icon"><IconSettings /></span>
            <span className="sidebar-item-label">设置</span>
          </button>
        }
      />

      <main className="main-content">
        {(projectError || toolsError) && <div className="error-banner">{projectError || toolsError}</div>}
        <div className="main-header">
          <div className="search-wrap">
            <IconSearch size={16} />
            <input
              className="search-input"
              type="text"
              placeholder="搜索项目..."
              value={searchQuery}
              onChange={e => setSearchQuery(e.target.value)}
            />
          </div>
          <button className="add-btn" onClick={() => setShowAdd(true)}>
            <IconPlus size={16} />
            新建项目
          </button>
        </div>

        <ProjectGrid
          projects={filteredProjects}
          gitStatusMap={gitStatusMap}
          isInstalled={isInstalled}
          onToolOpen={handleToolOpen}
          onEdit={handleEdit}
          onRemove={handleRemove}
          onToggleStar={handleToggleStar}
          emptyText={emptyTexts[activeView]}
          sessions={agentHook.sessions}
          agents={agentHook.agents}
          requirements={agentHook.requirements}
          onOpenAgent={setActiveAgentProject}
        />
      </main>

      {showAdd && (
        <AddProject onAdd={addProject} onClose={() => setShowAdd(false)} existingProjects={projects} />
      )}

      {showSettings && (
        <Settings tools={tools} agents={agentHook.agents} theme={theme} onThemeChange={setTheme} onClose={() => setShowSettings(false)} />
      )}

      {editingProject && (
        <EditProject
          project={editingProject}
          onSave={updateProject}
          onClose={() => setEditingProject(null)}
        />
      )}

      {activeAgentProject && (
        <AgentPanel
          project={activeAgentProject}
          sessions={agentHook.sessions}
          requirements={agentHook.requirements}
          agents={agentHook.agents}
          onClose={() => setActiveAgentProject(null)}
          spawnAgent={agentHook.spawnAgent}
          stopAgent={agentHook.stopAgent}
          addRequirement={agentHook.addRequirement}
          updateRequirement={agentHook.updateRequirement}
          getSessionsForProject={agentHook.getSessionsForProject}
          getRequirementsForProject={agentHook.getRequirementsForProject}
          recommendAgent={agentHook.recommendAgent}
        />
      )}
    </div>
    </ToastProvider>
  );
}

export default App;
