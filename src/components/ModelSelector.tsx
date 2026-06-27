import { useState, useRef, useEffect } from 'react';

export interface ModelOption {
  id: string;
  label: string;
  provider?: string;
}

// Bare fallback when ChatView passes no provider-sourced modelOptions. The real
// list comes from providers.toml (one option per enabled model); this only
// renders before that load, so it stays a single neutral entry instead of
// hardcoding one vendor's models.
const DEFAULT_MODELS: ModelOption[] = [
  { id: 'default', label: '默认模型', provider: '系统' },
];

interface ModelSelectorProps {
  value: string;
  onChange: (model: string) => void;
  models?: ModelOption[];
}

export function ModelSelector({ value, onChange, models = DEFAULT_MODELS }: ModelSelectorProps) {
  const [open, setOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  const current = models.find(m => m.id === value) ?? models[0];

  useEffect(() => {
    if (!open) return;
    const handleClick = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', handleClick);
    return () => document.removeEventListener('mousedown', handleClick);
  }, [open]);

  return (
    <div className="model-selector" style={{ position: 'relative' }} ref={dropdownRef}>
      <div
        onClick={() => setOpen(!open)}
        style={{ display: 'flex', alignItems: 'center', gap: 4, cursor: 'pointer', padding: '2px 6px' }}
      >
        <span style={{ fontSize: 11, color: 'var(--text-tertiary)' }}>模型</span>
        <span className="model-selector-name">{current.label}</span>
        <span style={{ fontSize: 10, color: 'var(--text-tertiary)' }}>▾</span>
      </div>
      {open && (
        <div className="model-selector-dropdown">
          {models.map(model => (
            <button
              key={model.id}
              className={`model-selector-item ${value === model.id ? 'active' : ''}`}
              onClick={() => { onChange(model.id); setOpen(false); }}
            >
              <span>{model.label}</span>
              {model.provider && (
                <span style={{ fontSize: 11, color: 'var(--text-muted)', marginLeft: 'auto' }}>
                  {model.provider}
                </span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
