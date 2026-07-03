import { useState, useEffect } from 'react';
import { useKnowledgeStore } from '../../stores/knowledgeStore';
import { useNavigationStore } from '../../stores/navigationStore';
import { useToast } from '../Toast';
import { Button } from '../ui/Button/Button';
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
 * ALL projects, expand long content, EDIT a wrong lesson in place (C1), or
 * delete one so it stops polluting the prompt. Backed by the existing
 * `knowledgeStore` (search / loadForProject / deleteEntry / updateEntry) → the
 * Rust commands `search_knowledge` / `get_knowledge_for_project` /
 * `delete_knowledge_entry` / `update_knowledge_entry`.
 */
export function MemorySection() {
  const { entries, searchResults, loading, loadForProject, search, deleteEntry, updateEntry } =
    useKnowledgeStore();
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

  const onUpdate = async (id: string, title: string, content: string) => {
    try {
      await updateEntry(id, title, content);
      success('记忆已更新');
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    }
  };

  const displayed: KnowledgeEntry[] = searched ? searchResults : entries;

  return (
    <div className="settings-section memory-section">
      <h3 className="settings-section-title">记忆</h3>
      <p className="settings-section-desc">
        智能体的跨会话长期记忆飞轮。任务结束时内核把高置信结论写入记忆，下次任务自动注入 system prompt 复用。这里可查看、搜索、编辑、删除已积累的记忆——改掉误学结论后下次注入的就是修正后的版本。
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
          <MemoryCard
            key={e.id}
            entry={e}
            onDelete={() => onDelete(e.id)}
            onUpdate={(title, content) => onUpdate(e.id, title, content)}
          />
        ))}
      </div>
    </div>
  );
}

function MemoryCard({
  entry,
  onDelete,
  onUpdate,
}: {
  entry: KnowledgeEntry;
  onDelete: () => void;
  onUpdate: (title: string, content: string) => Promise<void>;
}) {
  const [expanded, setExpanded] = useState(false);
  const [editing, setEditing] = useState(false);
  const [draftTitle, setDraftTitle] = useState(entry.title);
  const [draftContent, setDraftContent] = useState(entry.content);
  const [saving, setSaving] = useState(false);

  const long = entry.content.length > 200;
  const text = expanded || !long ? entry.content : `${entry.content.slice(0, 200)}…`;

  const startEdit = () => {
    // Reset the draft to the current entry each time editing opens, so a prior
    // canceled edit's stale draft can't bleed back in.
    setDraftTitle(entry.title);
    setDraftContent(entry.content);
    setEditing(true);
  };

  const cancelEdit = () => {
    setEditing(false);
    setDraftTitle(entry.title);
    setDraftContent(entry.content);
  };

  const save = async () => {
    // Empty title/content would wipe the lesson's usefulness — guard at the UI
    // edge (the backend accepts whatever it gets). Trim to avoid whitespace-only.
    if (!draftTitle.trim() || !draftContent.trim()) return;
    setSaving(true);
    try {
      await onUpdate(draftTitle, draftContent);
      setEditing(false);
    } catch {
      // Toast already surfaced the error; stay in editing mode so the user can retry.
    } finally {
      setSaving(false);
    }
  };

  if (editing) {
    return (
      <div className="memory-card memory-card-editing">
        <input
          className="memory-edit-title"
          aria-label="标题"
          value={draftTitle}
          onChange={(e) => setDraftTitle(e.target.value)}
        />
        <textarea
          className="memory-edit-content"
          aria-label="内容"
          rows={4}
          value={draftContent}
          onChange={(e) => setDraftContent(e.target.value)}
        />
        <div className="memory-card-actions">
          <Button size="sm" onClick={save} disabled={saving}>
            保存
          </Button>
          <Button variant="ghost" size="sm" onClick={cancelEdit} disabled={saving}>
            取消
          </Button>
        </div>
      </div>
    );
  }

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
        <Button
          variant="ghost"
          size="sm"
          onClick={startEdit}
          aria-label={`编辑记忆 ${entry.title || entry.id.slice(0, 8)}`}
        >
          编辑
        </Button>
        <Button
          variant="dangerGhost"
          size="sm"
          onClick={onDelete}
          aria-label={`删除记忆 ${entry.title || entry.id.slice(0, 8)}`}
        >
          删除
        </Button>
      </div>
    </div>
  );
}
