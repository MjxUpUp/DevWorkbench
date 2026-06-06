import { useState, useMemo } from 'react';
import { Sidebar } from './components/Sidebar';
import { ProjectGrid } from './components/ProjectGrid';
import { AddProject } from './components/AddProject';
import { Settings } from './components/Settings';
import { useProjects } from './hooks/useProjects';
import { useTools } from './hooks/useTools';
import type { Project } from './types';
import './styles/index.css';

const SIDEBAR_ITEMS = [
  { key: 'all', label: '全部项目', icon: '📂' },
  { key: 'recent', label: '最近打开', icon: '🕐' },
  { key: 'starred', label: '收藏', icon: '⭐' },
];

function App() {
  const [activeView, setActiveView] = useState('all');
  const [showAdd, setShowAdd] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [searchQuery, setSearchQuery] = useState('');

  const { projects, addProject, removeProject, updateProject, recordOpen } = useProjects();
  const { tools, isInstalled } = useTools();

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
    const newName = prompt('项目名称', project.name);
    if (newName === null) return;
    const newDesc = prompt('项目描述', project.description);
    if (newDesc === null) return;
    const newTags = prompt('标签（逗号分隔）', project.tags.join(', '));
    if (newTags === null) return;

    await updateProject(project.id, {
      name: newName,
      description: newDesc,
      tags: newTags.split(',').map(t => t.trim()).filter(Boolean),
    });
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
    <div className="app">
      <Sidebar
        items={SIDEBAR_ITEMS}
        activeKey={activeView}
        onSelect={setActiveView}
        footer={
          <button className="sidebar-item" onClick={() => setShowSettings(true)}>
            <span className="sidebar-item-icon">⚙️</span>
            <span className="sidebar-item-label">设置</span>
          </button>
        }
      />

      <main className="main-content">
        <div className="main-header">
          <input
            className="search-input"
            type="text"
            placeholder="搜索项目..."
            value={searchQuery}
            onChange={e => setSearchQuery(e.target.value)}
          />
          <button className="add-btn" onClick={() => setShowAdd(true)}>+ 新建项目</button>
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
        <AddProject onAdd={addProject} onClose={() => setShowAdd(false)} />
      )}

      {showSettings && (
        <Settings tools={tools} onClose={() => setShowSettings(false)} />
      )}
    </div>
  );
}

export default App;
