import { useState } from 'react';
import type { Requirement, Session, AgentInfo } from '../types';
import { RequirementCard } from './RequirementCard';
import { IconPlus } from './Icons';

interface RequirementListProps {
  requirements: Requirement[];
  sessions: Session[];
  agents: AgentInfo[];
  projectPath: string;
  onAdd: (title: string) => void;
  onStart: (id: string) => void;
  onMarkDone: (id: string) => void;
  onContinue: (id: string) => void;
}

type FilterKey = 'all' | 'todo' | 'in_progress' | 'done';

const FILTER_TABS: { key: FilterKey; label: string }[] = [
  { key: 'all', label: 'All' },
  { key: 'todo', label: 'Todo' },
  { key: 'in_progress', label: 'In Progress' },
  { key: 'done', label: 'Done' },
];

export function RequirementList({ requirements, sessions, agents, projectPath: _projectPath, onAdd, onStart, onMarkDone, onContinue }: RequirementListProps) {
  const [filter, setFilter] = useState<FilterKey>('all');
  const [newTitle, setNewTitle] = useState('');
  const [showAddForm, setShowAddForm] = useState(false);

  const filtered = filter === 'all'
    ? requirements
    : requirements.filter(r => r.status === filter);

  const handleAdd = () => {
    const title = newTitle.trim();
    if (!title) return;
    onAdd(title);
    setNewTitle('');
    setShowAddForm(false);
  };

  return (
    <div className="requirement-list">
      <div className="requirement-list-filters">
        {FILTER_TABS.map(tab => (
          <button
            key={tab.key}
            className={`requirement-filter-tab ${filter === tab.key ? 'active' : ''}`}
            onClick={() => setFilter(tab.key)}
          >
            {tab.label}
          </button>
        ))}
        <button
          className="requirement-add-btn"
          onClick={() => setShowAddForm(v => !v)}
          title="添加需求"
        >
          <IconPlus size={14} />
        </button>
      </div>

      {showAddForm && (
        <div className="requirement-add-form">
          <input
            className="requirement-add-input"
            type="text"
            placeholder="输入需求标题..."
            value={newTitle}
            onChange={e => setNewTitle(e.target.value)}
            onKeyDown={e => e.key === 'Enter' && handleAdd()}
            autoFocus
          />
          <button className="requirement-add-confirm" onClick={handleAdd} disabled={!newTitle.trim()}>
            添加
          </button>
        </div>
      )}

      <div className="requirement-list-items">
        {filtered.length === 0 ? (
          <div className="requirement-list-empty">暂无需求</div>
        ) : (
          filtered.map(req => (
            <RequirementCard
              key={req.id}
              requirement={req}
              sessions={sessions}
              agents={agents}
              onStart={onStart}
              onMarkDone={onMarkDone}
              onContinue={onContinue}
            />
          ))
        )}
      </div>
    </div>
  );
}
