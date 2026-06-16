import { useState, useRef, useEffect } from 'react';

export type AgentMode = 'default' | 'auto-edit' | 'plan' | 'silent' | 'skip-permissions';

interface ModeOption {
  id: AgentMode;
  label: string;
  shortLabel: string;
  desc: string;
}

const MODE_OPTIONS: ModeOption[] = [
  { id: 'default', label: '默认', shortLabel: '默认', desc: '交互式执行，询问关键操作' },
  { id: 'auto-edit', label: '自动', shortLabel: '自动', desc: '自动接受文件编辑不询问' },
  { id: 'plan', label: '计划', shortLabel: '计划', desc: '先输出计划，确认后执行' },
  { id: 'silent', label: '静默', shortLabel: '静默', desc: '最小化输出' },
  { id: 'skip-permissions', label: '跳过', shortLabel: '跳过', desc: '跳过所有权限检查' },
];

interface ModeSelectorProps {
  value: AgentMode;
  onChange: (mode: AgentMode) => void;
}

/**
 * Dropdown mode selector. Previously rendered as a flat row of 5 bare buttons
 * (`.mode-btn`, which had no CSS) — now drives the existing `.mode-selector-*`
 * dropdown styles in chat-view.css.
 */
export function ModeSelector({ value, onChange }: ModeSelectorProps) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  const current = MODE_OPTIONS.find((m) => m.id === value) ?? MODE_OPTIONS[0];

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);

  return (
    <div className="mode-selector" ref={ref}>
      <button
        type="button"
        className="mode-selector-trigger"
        onClick={() => setOpen((v) => !v)}
        title={current.label}
      >
        <span>{current.shortLabel}</span>
        <span className="mode-selector-chevron">▾</span>
      </button>
      {open && (
        <div className="mode-selector-dropdown" role="listbox">
          {MODE_OPTIONS.map((mode) => (
            <button
              key={mode.id}
              type="button"
              className={`mode-selector-item ${value === mode.id ? 'active' : ''}`}
              onClick={() => {
                onChange(mode.id);
                setOpen(false);
              }}
              role="option"
              aria-selected={value === mode.id}
            >
              <span className="mode-selector-item-label">{mode.label}</span>
              <span className="mode-selector-item-desc">{mode.desc}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
