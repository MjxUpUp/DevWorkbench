import { useState, useEffect, useRef } from 'react';

interface TriggerMenuItem {
  label: string;
  desc?: string;
  icon?: string;
  path?: string;
}

interface TriggerMenuProps {
  type: '@' | '/' | '$';
  position: { top: number; left: number };
  onSelect: (item: TriggerMenuItem) => void;
  onClose: () => void;
}

// Built-in commands for / trigger
const BUILTIN_COMMANDS: TriggerMenuItem[] = [
  { label: '/plan', desc: '计划模式 — 先输出计划再执行', icon: '📋' },
  { label: '/review', desc: '代码审查', icon: '🔍' },
  { label: '/test', desc: '运行测试', icon: '🧪' },
  { label: '/fix', desc: '修复问题', icon: '🔧' },
];

// Placeholder skills for $ trigger
const PLACEHOLDER_SKILLS: TriggerMenuItem[] = [
  { label: '新建功能', desc: '创建新功能实现', icon: '✨' },
  { label: '代码重构', desc: '重构已有代码', icon: '♻️' },
  { label: '性能优化', desc: '优化性能瓶颈', icon: '⚡' },
  { label: '安全审计', desc: '安全漏洞扫描', icon: '🛡️' },
];

export function TriggerMenu({ type, onSelect, onClose }: TriggerMenuProps) {
  const [search, setSearch] = useState('');
  const [items, setItems] = useState<TriggerMenuItem[]>([]);
  const searchRef = useRef<HTMLInputElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (type === '/') {
      setItems(BUILTIN_COMMANDS);
    } else if (type === '$') {
      setItems(PLACEHOLDER_SKILLS);
    } else if (type === '@') {
      // File listing — will be populated from backend when available
      setItems([
        { label: 'src/', desc: '源代码目录', path: 'src/', icon: '📁' },
        { label: 'package.json', desc: '项目配置', path: 'package.json', icon: '📄' },
        { label: 'Cargo.toml', desc: 'Rust 配置', path: 'Cargo.toml', icon: '📄' },
      ]);
    }
  }, [type]);

  useEffect(() => {
    searchRef.current?.focus();
  }, []);

  const filtered = search
    ? items.filter(i => i.label.toLowerCase().includes(search.toLowerCase()) || i.desc?.toLowerCase().includes(search.toLowerCase()))
    : items;

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Escape') {
      onClose();
    }
  };

  const headerLabel = type === '@' ? '附加文件' : type === '/' ? '命令' : '技能';

  return (
    <div className="trigger-menu" ref={menuRef}>
      <div className="trigger-menu-header">{headerLabel}</div>
      <input
        ref={searchRef}
        className="trigger-menu-search"
        placeholder="搜索..."
        value={search}
        onChange={e => setSearch(e.target.value)}
        onKeyDown={handleKeyDown}
      />
      {filtered.map((item, idx) => (
        <button
          key={idx}
          className="trigger-menu-item"
          onClick={() => onSelect(item)}
        >
          <span className="trigger-menu-item-icon">{item.icon || (type === '@' ? '📄' : type === '/' ? '⌘' : '⚡')}</span>
          <span className="trigger-menu-item-label">{item.label}</span>
          {item.desc && <span className="trigger-menu-item-desc">{item.desc}</span>}
        </button>
      ))}
      {filtered.length === 0 && (
        <div style={{ padding: '12px 16px', color: 'var(--text-muted)', fontSize: 13, textAlign: 'center' }}>
          无匹配结果
        </div>
      )}
    </div>
  );
}
