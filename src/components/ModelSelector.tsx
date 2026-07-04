import { useState, useRef, useEffect } from 'react';
import './ModelSelector.css';

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

/**
 * ModelSelector — 模型/供应商选择器。原为顶端 ChatHeader 内的裸文本触发器
 * (div onClick + 4 个从未定义的 className → 渲染成无样式纯文本)。下沉到 Composer
 * 左下 action bar 时一并补齐 CSS（见 ModelSelector.css）并把触发器改成 button
 * (a11y：原 div onClick 违反键盘可达红线)。弹层向上展开（Composer 在底部）。
 */
export function ModelSelector({ value, onChange, models = DEFAULT_MODELS }: ModelSelectorProps) {
  const [open, setOpen] = useState(false);
  const dropdownRef = useRef<HTMLDivElement>(null);

  const current = models.find((m) => m.id === value) ?? models[0];

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
    <div className={`model-selector${open ? ' open' : ''}`} ref={dropdownRef}>
      <button
        type="button"
        className="model-selector-trigger"
        aria-haspopup="listbox"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <span className="model-selector-label">模型</span>
        <span className="model-selector-name">{current.label}</span>
        <span className="model-selector-caret" aria-hidden="true">▾</span>
      </button>
      {open && (
        <div className="model-selector-dropdown" role="listbox">
          {models.map((model) => (
            <button
              key={model.id}
              type="button"
              role="option"
              aria-selected={value === model.id}
              className={`model-selector-item${value === model.id ? ' active' : ''}`}
              onClick={() => { onChange(model.id); setOpen(false); }}
            >
              <span className="model-selector-item-label">{model.label}</span>
              {model.provider && (
                <span className="model-selector-item-provider">{model.provider}</span>
              )}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
