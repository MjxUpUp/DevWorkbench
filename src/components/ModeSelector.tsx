export type AgentMode = 'default' | 'auto-edit' | 'plan' | 'silent' | 'skip-permissions';

interface ModeOption {
  id: AgentMode;
  label: string;
  shortLabel: string;
}

const MODE_OPTIONS: ModeOption[] = [
  { id: 'default', label: '默认模式 — 交互式执行，询问关键操作', shortLabel: '默认' },
  { id: 'auto-edit', label: '自动接受编辑 — 自动执行文件编辑不询问', shortLabel: '自动' },
  { id: 'plan', label: '计划模式 — 先输出计划，确认后执行', shortLabel: '计划' },
  { id: 'silent', label: '静默模式 — 最小化输出', shortLabel: '静默' },
  { id: 'skip-permissions', label: '跳过权限 — 跳过所有权限检查', shortLabel: '跳过' },
];

interface ModeSelectorProps {
  value: AgentMode;
  onChange: (mode: AgentMode) => void;
}

export function ModeSelector({ value, onChange }: ModeSelectorProps) {
  const current = MODE_OPTIONS.find(m => m.id === value) ?? MODE_OPTIONS[0];

  return (
    <div className="mode-selector" title={current.label}>
      {MODE_OPTIONS.map(mode => (
        <button
          key={mode.id}
          className={`mode-btn ${value === mode.id ? 'active' : ''}`}
          onClick={() => onChange(mode.id)}
          title={mode.label}
        >
          {mode.shortLabel}
        </button>
      ))}
    </div>
  );
}
