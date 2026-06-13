import { useState, useEffect } from 'react';
import { GeneralSection } from './GeneralSection';
import { AppearanceSection } from './AppearanceSection';
import { AgentSection } from './AgentSection';
import { ProvidersSection } from './ProvidersSection';
import { McpSection } from './McpSection';
import { BudgetSection } from './BudgetSection';
import { SkillsSection } from './SkillsSection';
import { useSettingsStore } from '../../stores/settingsStore';
import {
  IconSettings, IconSun, IconTerminal, IconCpu,
  IconSparkles, IconBrain, IconStar,
} from '../Icons';

export type SettingsSection = 'general' | 'appearance' | 'agent' | 'providers' | 'mcp' | 'budget' | 'skills';

const SECTIONS: { id: SettingsSection; label: string; Icon: React.FC<{ size?: number; className?: string }> }[] = [
  { id: 'general', label: '通用', Icon: IconSettings },
  { id: 'appearance', label: '外观', Icon: IconSun },
  { id: 'agent', label: 'Agent 管理', Icon: IconTerminal },
  { id: 'providers', label: '模型供应商', Icon: IconCpu },
  { id: 'mcp', label: 'MCP 服务器', Icon: IconBrain },
  { id: 'budget', label: '预算', Icon: IconStar },
  { id: 'skills', label: '技能', Icon: IconSparkles },
];

export function SettingsView() {
  const [activeSection, setActiveSection] = useState<SettingsSection>('general');
  const loadSettings = useSettingsStore((s) => s.loadSettings);

  // Load settings once on mount — shared store eliminates per-section loading
  useEffect(() => { loadSettings(); }, [loadSettings]);

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
            <span className="settings-nav-icon"><section.Icon size={16} /></span>
            {section.label}
          </button>
        ))}
      </div>
      <div className="settings-view-content">
        {/* Use key to preserve local state within each section when switching */}
        {activeSection === 'general' && <GeneralSection key="general" />}
        {activeSection === 'appearance' && <AppearanceSection key="appearance" />}
        {activeSection === 'agent' && <AgentSection key="agent" />}
        {activeSection === 'providers' && <ProvidersSection key="providers" />}
        {activeSection === 'mcp' && <McpSection key="mcp" />}
        {activeSection === 'budget' && <BudgetSection key="budget" />}
        {activeSection === 'skills' && <SkillsSection key="skills" />}
      </div>
    </div>
  );
}
