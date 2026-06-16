import { useState, useEffect, useRef } from 'react';
import { useNavigationStore } from '../stores/navigationStore';
import { useAgentStore } from '../stores/agentStore';
import { useProjectStore } from '../stores/projectStore';
import { useKnowledgeStore } from '../stores/knowledgeStore';

type ResultKind = 'command' | 'project' | 'conversation' | 'knowledge';

interface ResultItem {
  kind: ResultKind;
  /** Main label. */
  label: string;
  /** Secondary line (path / agent / category) — optional. */
  secondary?: string;
  /** Action on pick. Knowledge hits have none (informational, like SearchView). */
  action?: () => void;
}

/**
 * Transparent overlay + centered input — the single search surface (问题3).
 *
 * Replaces the legacy persistent SearchView, consolidating all retrieval into
 * one modal: commands, projects, conversations, and the backend knowledge base.
 * The knowledge dimension is what the old SearchView added and the bare palette
 * lacked — without it the 搜索 button lost project + knowledge search when it
 * was rewired here. Open via the sidebar 搜索 item or Ctrl/Cmd+K.
 */
export function CommandPalette() {
  const open = useNavigationStore((s) => s.commandPaletteOpen);
  const setOpen = useNavigationStore((s) => s.setCommandPaletteOpen);
  const setActiveView = useNavigationStore((s) => s.setActiveView);
  const selectProject = useNavigationStore((s) => s.selectProject);
  const selectConversation = useNavigationStore((s) => s.selectConversation);

  const conversations = useAgentStore((s) => s.conversations);
  const projects = useProjectStore((s) => s.projects);
  const knowledgeResults = useKnowledgeStore((s) => s.searchResults);
  const searchKnowledge = useKnowledgeStore((s) => s.search);

  const [query, setQuery] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setQuery('');
      inputRef.current?.focus();
    }
  }, [open]);

  // Debounced backend knowledge search — only with a real query, mirroring the
  // old SearchView's behavior. The palette is transient, so clear on close.
  useEffect(() => {
    const q = query.trim();
    if (!q) return;
    const id = setTimeout(() => { searchKnowledge(q, 20); }, 250);
    return () => clearTimeout(id);
  }, [query, searchKnowledge]);

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if ((e.metaKey || e.ctrlKey) && e.key === 'k') {
        e.preventDefault();
        setOpen(!open);
      }
      if (e.key === 'Escape' && open) {
        setOpen(false);
      }
    };
    window.addEventListener('keydown', handler);
    return () => window.removeEventListener('keydown', handler);
  }, [open, setOpen]);

  if (!open) return null;

  const close = () => setOpen(false);
  const q = query.trim().toLowerCase();

  const commands: ResultItem[] = [
    { kind: 'command', label: '创建任务', action: () => { setActiveView('task'); close(); } },
    { kind: 'command', label: '技能', action: () => { setActiveView('skills'); close(); } },
    { kind: 'command', label: '设置', action: () => { setActiveView('settings'); close(); } },
  ];

  const projectItems: ResultItem[] = (q
    ? projects.filter((p) => p.name.toLowerCase().includes(q) || p.path.toLowerCase().includes(q))
    : []
  ).map((p) => ({
    kind: 'project' as const,
    label: p.name,
    secondary: p.path,
    action: () => {
      selectProject(p);
      selectConversation(null);
      setActiveView('task');
      close();
    },
  }));

  // Conversations are the navigable unit — dedup from the flat turn list.
  const conversationItems: ResultItem[] = (q
    ? conversations.filter((c) => c.title.toLowerCase().includes(q))
    : conversations
  ).slice(0, 5).map((c) => ({
    kind: 'conversation' as const,
    label: c.title,
    secondary: `${c.lastAgent ?? ''} · ${c.status}`,
    action: () => {
      // Selecting a conversation requires its project to be active first,
      // otherwise the sidebar/ChatView scope won't match.
      const proj = projects.find((p) => p.path === c.projectPath);
      if (proj) selectProject(proj);
      setActiveView('task');
      selectConversation(c.id);
      close();
    },
  }));

  const knowledgeItems: ResultItem[] = (q ? knowledgeResults : []).map((k) => ({
    kind: 'knowledge' as const,
    label: k.title,
    secondary: `${k.category} · 置信度 ${Math.round(k.confidence * 100)}%`,
  }));

  const commandItems = q ? commands.filter((c) => c.label.toLowerCase().includes(q)) : commands;

  const grouped: { kind: ResultKind; title: string; items: ResultItem[] }[] = [];
  if (commandItems.length) grouped.push({ kind: 'command', title: '操作', items: commandItems });
  if (projectItems.length) grouped.push({ kind: 'project', title: '项目', items: projectItems });
  if (conversationItems.length) grouped.push({ kind: 'conversation', title: '对话', items: conversationItems });
  if (knowledgeItems.length) grouped.push({ kind: 'knowledge', title: '知识', items: knowledgeItems });

  const flat = grouped.flatMap((g) => g.items);
  const firstActionable = flat.find((i) => i.action);

  return (
    <div className="command-palette-overlay" onClick={close}>
      <div className="command-palette" onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          className="command-palette-input"
          type="text"
          placeholder="搜索操作、项目、对话、知识…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && firstActionable) {
              firstActionable.action!();
            }
          }}
        />
        <div className="command-palette-results">
          {grouped.length === 0 && (
            <div className="command-palette-empty">{q ? '无匹配结果' : '输入关键词以检索…'}</div>
          )}
          {grouped.map((g) => (
            <div key={g.kind} className="command-palette-group">
              <div className="command-palette-group-title">{g.title}</div>
              {g.items.map((item, i) =>
                item.action ? (
                  <button
                    key={`${g.kind}-${i}`}
                    className="command-palette-item"
                    onClick={item.action}
                  >
                    <span className="command-palette-item-primary">{item.label}</span>
                    {item.secondary && (
                      <span className="command-palette-item-secondary">{item.secondary}</span>
                    )}
                  </button>
                ) : (
                  <div key={`${g.kind}-${i}`} className="command-palette-item command-palette-item--static" title={item.label}>
                    <span className="command-palette-item-primary">{item.label}</span>
                    {item.secondary && (
                      <span className="command-palette-item-secondary">{item.secondary}</span>
                    )}
                  </div>
                ),
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
