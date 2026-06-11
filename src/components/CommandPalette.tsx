import { useState, useEffect, useRef } from 'react';
import { useNavigationStore } from '../stores/navigationStore';
import { useAgentStore } from '../stores/agentStore';

export function CommandPalette() {
  const open = useNavigationStore((s) => s.commandPaletteOpen);
  const setOpen = useNavigationStore((s) => s.setCommandPaletteOpen);
  const setActiveTab = useNavigationStore((s) => s.setActiveTab);
  const sessions = useAgentStore((s) => s.sessions);

  const [query, setQuery] = useState('');
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setQuery('');
      inputRef.current?.focus();
    }
  }, [open]);

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

  const commands = [
    { label: '概览', action: () => { setActiveTab('overview'); setOpen(false); } },
    { label: '对话', action: () => { setActiveTab('sessions'); setOpen(false); } },
    { label: '时间线', action: () => { setActiveTab('timeline'); setOpen(false); } },
    { label: '知识库', action: () => { setActiveTab('knowledge'); setOpen(false); } },
  ];

  // Add recent sessions as quick-access items
  const recentSessions = sessions
    .filter((s) => query ? s.prompt.toLowerCase().includes(query.toLowerCase()) : true)
    .slice(0, 5)
    .map((s) => ({
      label: `对话: ${s.prompt.slice(0, 40)}`,
      action: () => {
        setActiveTab('sessions');
        useNavigationStore.getState().selectSession(s.id);
        setOpen(false);
      },
    }));

  const filtered = query
    ? commands.filter((c) => c.label.includes(query)).concat(recentSessions)
    : commands;

  return (
    <div className="command-palette-overlay" onClick={() => setOpen(false)}>
      <div className="command-palette" onClick={(e) => e.stopPropagation()}>
        <input
          ref={inputRef}
          className="command-palette-input"
          type="text"
          placeholder="搜索命令、对话..."
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && filtered.length > 0) {
              filtered[0].action();
            }
          }}
        />
        <div className="command-palette-results">
          {filtered.map((item, i) => (
            <button key={i} className="command-palette-item" onClick={item.action}>
              {item.label}
            </button>
          ))}
          {filtered.length === 0 && (
            <div className="command-palette-empty">无匹配结果</div>
          )}
        </div>
      </div>
    </div>
  );
}
