import type { ReactNode } from 'react';

export interface SidebarItem {
  key: string;
  label: string;
  icon: string;
}

interface SidebarProps {
  items: SidebarItem[];
  activeKey: string;
  onSelect: (key: string) => void;
  footer?: ReactNode;
}

export function Sidebar({ items, activeKey, onSelect, footer }: SidebarProps) {
  return (
    <aside className="sidebar">
      <div className="sidebar-header">
        <h1 className="sidebar-title">一目了然</h1>
      </div>
      <nav className="sidebar-nav">
        {items.map(item => (
          <button
            key={item.key}
            className={`sidebar-item ${activeKey === item.key ? 'active' : ''}`}
            onClick={() => onSelect(item.key)}
          >
            <span className="sidebar-item-icon">{item.icon}</span>
            <span className="sidebar-item-label">{item.label}</span>
          </button>
        ))}
      </nav>
      {footer && <div className="sidebar-footer">{footer}</div>}
    </aside>
  );
}
