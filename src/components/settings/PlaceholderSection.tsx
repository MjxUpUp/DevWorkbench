import type { FC } from 'react';

type SettingsIcon = FC<{ size?: number; className?: string }>;

interface PlaceholderSectionProps {
  title: string;
  desc: string;
  hint?: string;
  Icon?: SettingsIcon;
}

/**
 * Placeholder for settings categories that exist in zcode but are not yet
 * implemented here. Reuses the shared `.settings-coming-soon` aesthetic so
 * every category — built or pending — shares the same visual language.
 */
export function PlaceholderSection({ title, desc, hint, Icon }: PlaceholderSectionProps) {
  return (
    <div className="settings-section">
      <h3 className="settings-section-title">{title}</h3>
      <p className="settings-section-desc">{desc}</p>
      <div className="settings-coming-soon">
        <div className="settings-coming-soon-icon">{Icon ? <Icon size={32} /> : null}</div>
        <p>即将推出</p>
        <span>{hint ?? `${title}功能正在开发中，敬请期待`}</span>
      </div>
    </div>
  );
}
