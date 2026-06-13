import { useState, useMemo, useEffect, useRef } from 'react';
import { useNavigationStore } from '../../stores/navigationStore';
import { useProjectStore } from '../../stores/projectStore';
import { useAgentStore } from '../../stores/agentStore';
import { useKnowledgeStore } from '../../stores/knowledgeStore';
import { IconSearch } from '../Icons';

/**
 * Persistent search view — project / conversation / knowledge retrieval.
 *
 * This is the always-available form of what the CommandPalette did as a modal.
 * Selecting a project switches to the task view with it active; selecting a
 * session opens it; selecting a knowledge entry is informational (no deep link
 * yet). Knowledge search runs against the backend when the query is non-empty.
 */
export function SearchView() {
  const [query, setQuery] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  const projects = useProjectStore((s) => s.projects);
  const sessions = useAgentStore((s) => s.sessions);
  const knowledgeResults = useKnowledgeStore((s) => s.searchResults);
  const knowledgeLoading = useKnowledgeStore((s) => s.loading);
  const searchKnowledge = useKnowledgeStore((s) => s.search);

  const selectProject = useNavigationStore((s) => s.selectProject);
  const selectSession = useNavigationStore((s) => s.selectSession);
  const setActiveView = useNavigationStore((s) => s.setActiveView);

  // Kick off backend knowledge search (debounced) only with a real query.
  useEffect(() => {
    const q = query.trim();
    if (!q) return;
    const id = setTimeout(() => { searchKnowledge(q, 20); }, 250);
    return () => clearTimeout(id);
  }, [query, searchKnowledge]);

  const q = query.trim().toLowerCase();

  const projectHits = useMemo(
    () => q ? projects.filter((p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q)) : [],
    [projects, q],
  );

  const sessionHits = useMemo(
    () => q ? sessions.filter((s) => s.prompt.toLowerCase().includes(q)).slice(0, 20) : [],
    [sessions, q],
  );

  const knowledgeHits = q ? knowledgeResults : [];
  const empty = q && projectHits.length === 0 && sessionHits.length === 0 && knowledgeHits.length === 0 && !knowledgeLoading;

  const handlePickProject = (projectId: string) => {
    const project = projects.find((p) => p.id === projectId);
    if (!project) return;
    selectProject(project);
    selectSession(null);
    setActiveView('task');
  };

  const handlePickSession = (sessionId: string) => {
    selectSession(sessionId);
  };

  return (
    <div className="search-view">
      <div className="search-view-bar">
        <IconSearch size={16} className="search-view-icon" />
        <input
          ref={inputRef}
          className="search-view-input"
          type="text"
          placeholder="搜索项目、对话或知识…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          autoFocus
        />
      </div>

      <div className="search-view-results">
        {!q && (
          <div className="search-view-hint">
            输入关键词以检索项目、对话与知识库。
          </div>
        )}

        {empty && (
          <div className="search-view-hint">无匹配结果。</div>
        )}

        {projectHits.length > 0 && (
          <section className="search-section">
            <h3 className="search-section-title">项目</h3>
            {projectHits.map((p) => (
              <button key={p.id} className="search-result-item" onClick={() => handlePickProject(p.id)}>
                <span className="search-result-primary">{p.name}</span>
                <span className="search-result-secondary">{p.path}</span>
              </button>
            ))}
          </section>
        )}

        {sessionHits.length > 0 && (
          <section className="search-section">
            <h3 className="search-section-title">对话</h3>
            {sessionHits.map((s) => (
              <button key={s.id} className="search-result-item" onClick={() => handlePickSession(s.id)}>
                <span className="search-result-primary">{s.prompt.slice(0, 80)}</span>
                <span className="search-result-secondary">{s.agentType} · {s.status}</span>
              </button>
            ))}
          </section>
        )}

        {knowledgeHits.length > 0 && (
          <section className="search-section">
            <h3 className="search-section-title">知识</h3>
            {knowledgeHits.map((k) => (
              <div key={k.id} className="search-result-item" title={k.content}>
                <span className="search-result-primary">{k.title}</span>
                <span className="search-result-secondary">{k.category} · 置信度 {Math.round(k.confidence * 100)}%</span>
              </div>
            ))}
          </section>
        )}
      </div>
    </div>
  );
}
