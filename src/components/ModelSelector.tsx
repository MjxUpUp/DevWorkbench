import { useState, useRef, useEffect } from 'react';

export interface ModelOption {
  id: string;
  label: string;
  provider?: string;
}

// Default model options — will be populated from providers config when available
const DEFAULT_MODELS: ModelOption[] = [
  { id: 'default', label: '默认模型', provider: '系统' },
  { id: 'claude-opus-4-8', label: 'Claude Opus 4.8', provider: 'Anthropic' },
  { id: 'claude-sonnet-4-6', label: 'Claude Sonnet 4.6', provider: 'Anthropic' },
  { id: 'glm-5.1', label: 'GLM-5.1', provider: 'Z.AI' },
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
