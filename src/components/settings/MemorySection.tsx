import { useState, useEffect } from 'react';
import { useKnowledgeStore } from '../../stores/knowledgeStore';
import { useNavigationStore } from '../../stores/navigationStore';
import { useToast } from '../Toast';
import type { KnowledgeEntry } from '../../types';

/**
 * 记忆管理 — the user-facing surface for the v1.3-T2 long-term memory flywheel.
 *
 * The kernel writes high-confidence session conclusions into `knowledge_entries`
 * at session end, then `memory_prompt_suffix` re-injects them into the next
 * session's system prompt. Until this section existed that loop was invisible:
 * a mislearned lesson stayed injected forever with no way to see or remove it.
 *
 * This view closes the gap — list the current project's memories, search across
 * ALL projects, expand long content, and delete wrong entries so they stop
 * polluting the prompt. Backed by the existing `knowledgeStore` (search /
 * loadForProject / deleteEntry) → the already-registered Rust commands
 * `search_knowledge` / `get_knowledge_for_project` / `delete_knowledge_entry`.
 */
export function MemorySection() {
  const { entries, searchResults, loading, loadForProject, search, deleteEntry } = useKnowledgeStore();
  const activeProject = useNavigationStore((s) => s.activeProject);
  const { success, error } = useToast();
  const [query, setQuery] = useState('');
  const [searched, setSearched] = useState(false);

  // Load the active project's memories on mount + whenever the project changes.
  useEffect(() => {
    if (activeProject) loadForProject(activeProject.path);
  }, [activeProject, loadForProject]);

  const onSearch = async (q: string) => {
    setQuery(q);
    if (q.trim()) {
      await search(q);
      setSearched(true);
    } else {
      // Empty query → back to the project-scoped list.
      setSearched(false);
    }
  };

  const onDelete = async (id: string) => {
    try {
      await deleteEntry(id);
      success('记忆已删除，下次任务不再注入');
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    }
  };

  const displayed: KnowledgeEntry[] = searched ? searchResults : entries;

  return (
    <div className="settings-section memory-section">
      <h3 className="settings-section-title">记忆</h3>
      <p className="settings-section-desc">
        智能体的跨会话长期记忆飞轮。任务结束时内核把高置信结论写入记忆，下次任务自动注入 system prompt 复用。这里可查看、搜索、删除已积累的记忆——删掉误学结论后不再注入。
      </p>

      <div className="memory-search-row">
        <input
          className="memory-search-input"
          placeholder={activeProject ? `搜索全局记忆（清空查看「${activeProject.name}」项目记忆）` : '搜索全局记忆'}
          value={query}
          onChange={(e) => onSearch(e.target.value)}
          aria-label="搜索记忆"
        />
        {searched && <span className="memory-search-hint">全局结果 {searchResults.length} 条</span>}
      </div>

      {!searched && activeProject && (
        <p className="memory-scope-hint">
          当前：项目「{activeProject.name}」记忆 {entries.length} 条 · 在上方输入可切换全局搜索
        </p>
      )}

      {loading && <p className="settings-section-desc">加载中...</p>}

      {!loading && displayed.length === 0 && (
        <div className="memory-empty">
          <p>
            {searched
              ? '没有匹配的记忆'
              : activeProject
                ? '该项目暂无记忆——完成任务后内核会自动积累'
                : '暂无记忆'}
          </p>
        </div>
      )}

      <div className="memory-list">
        {displayed.map((e) => (
          <MemoryCard key={e.id} entry={e} onDelete={() => onDelete(e.id)} />
        ))}
      </div>
    </div>
  );
}

function MemoryCard({ entry, onDelete }: { entry: KnowledgeEntry; onDelete: () => void }) {
  const [expanded, setExpanded] = useState(false);
  const long = entry.content.length > 200;
  const text = expanded || !long ? entry.content : `${entry.content.slice(0, 200)}…`;

  return (
    <div className="memory-card">
      <div className="memory-card-header">
        <span className="memory-card-title">{entry.title || '(无标题)'}</span>
        <span className={`memory-card-category cat-${entry.category}`}>{entry.category}</span>
        <span className="memory-card-confidence" title="置信度，越高越可能被注入">
          {(entry.confidence * 100).toFixed(0)}%
        </span>
      </div>
      <p
        className={`memory-card-content${long ? ' expandable' : ''}`}
        onClick={() => long && setExpanded((v) => !v)}
      >
        {text}
      </p>
      <div className="memory-card-meta">
        <span>{entry.sourceAgent}</span>
        {entry.sourceType && <span>· {entry.sourceType}</span>}
        <span>· {(entry.createdAt || '').slice(0, 10)}</span>
        <button
          className="memory-card-delete"
          onClick={onDelete}
          aria-label={`删除记忆 ${entry.title || entry.id.slice(0, 8)}`}
        >
          删除
        </button>
      </div>
    </div>
  );
}
