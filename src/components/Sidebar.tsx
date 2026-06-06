import type { ReactNode, ComponentType } from 'react';

export interface SidebarItem {
  key: string;
  label: string;
  IconComponent: ComponentType<{ size?: number; className?: string }>;
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
        <div className="sidebar-brand">
          <div className="sidebar-logo">DW</div>
          <div>
            <div className="sidebar-title">一目了然</div>
            <div className="sidebar-subtitle">Dev Workbench</div>
          </div>
        </div>
      </div>
      <nav className="sidebar-nav">
        {items.map(item => (
          <button
            key={item.key}
            className={`sidebar-item ${activeKey === item.key ? 'active' : ''}`}
            onClick={() => onSelect(item.key)}
          >
            <span className="sidebar-item-icon">
              <item.IconComponent size={18} />
            </span>
            <span className="sidebar-item-label">{item.label}</span>
          </button>
        ))}
      </nav>
      {footer && <div className="sidebar-footer">{footer}</div>}
    </aside>
  );
}
