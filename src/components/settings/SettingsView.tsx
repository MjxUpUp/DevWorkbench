import { useState } from 'react';
import { GeneralSection } from './GeneralSection';
import { AppearanceSection } from './AppearanceSection';
import { AgentSection } from './AgentSection';
import { ProvidersSection } from './ProvidersSection';
import { McpSection } from './McpSection';
import { BudgetSection } from './BudgetSection';
import { SkillsSection } from './SkillsSection';

export type SettingsSection = 'general' | 'appearance' | 'agent' | 'providers' | 'mcp' | 'budget' | 'skills';

const SECTIONS: { id: SettingsSection; label: string; icon: string }[] = [
  { id: 'general', label: '通用', icon: '⚙️' },
  { id: 'appearance', label: '外观', icon: '🎨' },
  { id: 'agent', label: 'Agent 管理', icon: '🤖' },
  { id: 'providers', label: '模型供应商', icon: '🧠' },
  { id: 'mcp', label: 'MCP 服务器', icon: '🔌' },
  { id: 'budget', label: '预算', icon: '💰' },
  { id: 'skills', label: '技能', icon: '⚡' },
];

export function SettingsView() {
  const [activeSection, setActiveSection] = useState<SettingsSection>('general');

  const renderSection = () => {
    switch (activeSection) {
      case 'general':
        return <GeneralSection />;
      case 'appearance':
        return <AppearanceSection />;
      case 'agent':
        return <AgentSection />;
      case 'providers':
        return <ProvidersSection />;
      case 'mcp':
        return <McpSection />;
      case 'budget':
        return <BudgetSection />;
      case 'skills':
        return <SkillsSection />;
    }
  };

  return (
    <div className="settings-view">
      <div className="settings-view-nav">
        <div className="settings-view-nav-header">
          <h2>设置</h2>
        </div>
        {SECTIONS.map(section => (
          <button
            key={section.id}
            className={`settings-nav-item ${activeSection === section.id ? 'active' : ''}`}
            onClick={() => setActiveSection(section.id)}
          >
            <span className="settings-nav-icon">{section.icon}</span>
            {section.label}
          </button>
        ))}
      </div>
      <div className="settings-view-content">
        {renderSection()}
      </div>
    </div>
  );
}
