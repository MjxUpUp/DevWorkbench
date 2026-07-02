import { useState, useRef, useEffect } from 'react';

// 'executing' is Mission Phase 2 (D4) — set internally by the `mission_apply`
// command, not surfaced in MODE_OPTIONS for manual selection. It round-trips
// the backend PermissionMode::Executing wire value.
export type AgentMode = 'default' | 'auto-edit' | 'plan' | 'executing' | 'dry-run' | 'silent' | 'skip-permissions' | 'human-gate';

interface ModeOption {
  id: AgentMode;
  label: string;
  shortLabel: string;
  desc: string;
}

const MODE_OPTIONS: ModeOption[] = [
  { id: 'default', label: '默认', shortLabel: '默认', desc: '交互式执行，询问关键操作' },
  { id: 'human-gate', label: '人工审批', shortLabel: '审批', desc: '破坏性操作（删文件/强推等）执行前弹窗，需人工同意' },
  { id: 'auto-edit', label: '自动', shortLabel: '自动', desc: '自动接受文件编辑不询问' },
  { id: 'plan', label: '计划', shortLabel: '计划', desc: '先输出计划，确认后执行' },
  { id: 'dry-run', label: '预演', shortLabel: '预演', desc: '预演执行计划：只读工具真跑、写入类工具不落地' },
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
        data-testid="mode-selector-trigger"
      >
        <span>{current.shortLabel}</span>
        <span className="mode-selector-chevron">▾</span>
      </button>
      {open && (
        <div className="mode-selector-dropdown" role="listbox" data-testid="mode-selector-dropdown">
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
