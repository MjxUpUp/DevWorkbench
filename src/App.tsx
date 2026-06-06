import { useState, useMemo } from 'react';
import { Sidebar } from './components/Sidebar';
import { ProjectGrid } from './components/ProjectGrid';
import { AddProject } from './components/AddProject';
import { EditProject } from './components/EditProject';
import { Settings } from './components/Settings';
import { ToastProvider } from './components/Toast';
import { useProjects } from './hooks/useProjects';
import { useTools } from './hooks/useTools';
import { IconSearch, IconPlus, IconFolder, IconClock, IconStar, IconSettings } from './components/Icons';
import type { Project } from './types';
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

  const { projects, addProject, removeProject, updateProject, recordOpen, error: projectError } = useProjects();
  const { tools, isInstalled, error: toolsError } = useTools();

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

  const handleOpen = async (id: string) => {
    await recordOpen(id);
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
          isInstalled={isInstalled}
          onOpen={handleOpen}
          onEdit={handleEdit}
          onRemove={handleRemove}
          onToggleStar={handleToggleStar}
          emptyText={emptyTexts[activeView]}
        />
      </main>

      {showAdd && (
        <AddProject onAdd={addProject} onClose={() => setShowAdd(false)} existingProjects={projects} />
      )}

      {showSettings && (
        <Settings tools={tools} onClose={() => setShowSettings(false)} />
      )}

      {editingProject && (
        <EditProject
          project={editingProject}
          onSave={updateProject}
          onClose={() => setEditingProject(null)}
        />
      )}
    </div>
    </ToastProvider>
  );
}

export default App;
