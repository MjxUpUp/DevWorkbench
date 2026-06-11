import { useState, useEffect } from 'react';
import type { Project } from '../../types';
import { useKnowledgeStore } from '../../stores/knowledgeStore';
import { KnowledgeCard } from '../KnowledgeCard';

interface KnowledgeTabProps {
  project: Project | null;
}

export function KnowledgeTab({ project }: KnowledgeTabProps) {
  const entries = useKnowledgeStore((s) => s.entries);
  const searchResults = useKnowledgeStore((s) => s.searchResults);
  const loading = useKnowledgeStore((s) => s.loading);
  const loadForProject = useKnowledgeStore((s) => s.loadForProject);
  const search = useKnowledgeStore((s) => s.search);
  const deleteEntry = useKnowledgeStore((s) => s.deleteEntry);

  const [query, setQuery] = useState('');
  const [isSearch, setIsSearch] = useState(false);

  useEffect(() => {
    if (project) {
      loadForProject(project.path);
    }
  }, [project, loadForProject]);

  const handleSearch = () => {
    if (query.trim()) {
      search(query.trim());
      setIsSearch(true);
    } else {
      setIsSearch(false);
    }
  };

  const displayed = isSearch ? searchResults : entries;

  return (
    <div className="knowledge-tab">
      <div className="knowledge-search">
        <input
          className="knowledge-search-input"
          type="text"
          placeholder="搜索知识库..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => { if (e.key === 'Enter') handleSearch(); }}
        />
        <button className="knowledge-search-btn" onClick={handleSearch}>搜索</button>
        {isSearch && (
          <button className="knowledge-search-clear" onClick={() => { setQuery(''); setIsSearch(false); }}>清除</button>
        )}
      </div>

      {loading && <div className="knowledge-loading">加载中...</div>}

      {!loading && displayed.length === 0 && (
        <div className="tab-content-empty">
          <div className="tab-content-empty-icon">⬡</div>
          <h2>暂无知识条目</h2>
          <p>Agent 完成对话后将自动采集知识到此处</p>
        </div>
      )}

      <div className="knowledge-entries">
        {displayed.map((entry) => (
          <KnowledgeCard
            key={entry.id}
            entry={entry}
            onDelete={() => deleteEntry(entry.id)}
          />
        ))}
      </div>
    </div>
  );
}
