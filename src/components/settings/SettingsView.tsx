import { useState, useEffect } from 'react';
import { AppearanceSection } from './AppearanceSection';
import { AgentSection } from './AgentSection';
import { ProvidersSection } from './ProvidersSection';
import { McpSection } from './McpSection';
import { MemorySection } from './MemorySection';
import { SkillsSection } from './SkillsSection';
import { UsageStatsSection } from './UsageStatsSection';
import { PlaceholderSection } from './PlaceholderSection';
import { PluginsSection } from './PluginsSection';
import { CommandsSection } from './CommandsSection';
import { useSettingsStore } from '../../stores/settingsStore';
import {
  IconSun, IconTerminal, IconCpu,
  IconSparkles, IconBrain, IconInbox,
  IconUser, IconPlay, IconEdit, IconStar,
  IconDashboard, IconChat, IconX,
} from '../Icons';
import { useNavigationStore } from '../../stores/navigationStore';

type SettingsIcon = React.FC<{ size?: number; className?: string }>;

/**
 * Settings categories. Slimmed down from the earlier 15-section list:
 *   - removed "AI 配置" (was GeneralSection — didn't match the label's intent)
 *   - removed "代码预览" (placeholder)
 *   - removed "索引" (placeholder)
 * Remaining sections map to a real component or share the PlaceholderSection
 * aesthetic.
 */
export type SettingsSection =
  | 'agent-tools' | 'providers' | 'plugins'
  | 'skills' | 'mcp' | 'sub-agents' | 'commands' | 'hooks'
  | 'memory' | 'output-style' | 'usage-stats' | 'onboarding';

interface SectionDef {
  id: SettingsSection;
  label: string;
  Icon: SettingsIcon;
  Component?: React.FC;
  placeholder?: { title: string; desc: string; hint?: string };
}

const SECTIONS: SectionDef[] = [
  { id: 'agent-tools', label: '智能体工具', Icon: IconTerminal, Component: AgentSection },
  { id: 'providers', label: '模型供应商', Icon: IconCpu, Component: ProvidersSection },
  { id: 'plugins', label: '能力总览', Icon: IconInbox, Component: PluginsSection },
  { id: 'skills', label: '技能', Icon: IconSparkles, Component: SkillsSection },
  { id: 'mcp', label: 'MCP 服务器', Icon: IconBrain, Component: McpSection },
  { id: 'sub-agents', label: '子智能体', Icon: IconUser, placeholder: { title: '子智能体', desc: '配置可被主智能体调用的子智能体', hint: '子智能体配置正在开发中，敬请期待' } },
  { id: 'commands', label: '命令', Icon: IconPlay, Component: CommandsSection },
  { id: 'hooks', label: '钩子', Icon: IconEdit, placeholder: { title: '钩子', desc: '配置生命周期钩子与事件回调', hint: '钩子配置正在开发中，敬请期待' } },
  { id: 'memory', label: '记忆', Icon: IconStar, Component: MemorySection },
  { id: 'output-style', label: '输出样式', Icon: IconSun, Component: AppearanceSection },
  { id: 'usage-stats', label: '使用统计', Icon: IconDashboard, Component: UsageStatsSection },
  { id: 'onboarding', label: '引导', Icon: IconChat, placeholder: { title: '引导', desc: '新手引导与帮助文档', hint: '引导功能正在开发中，敬请期待' } },
];

export function SettingsView() {
  const [activeSection, setActiveSection] = useState<SettingsSection>('agent-tools');
  const loadSettings = useSettingsStore((s) => s.loadSettings);
  const setActiveView = useNavigationStore((s) => s.setActiveView);

  // Load settings once on mount — shared store eliminates per-section loading
  useEffect(() => { loadSettings(); }, [loadSettings]);

  // ESC closes the settings overlay (returns to the task view).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setActiveView('task');
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [setActiveView]);

  const active = SECTIONS.find((s) => s.id === activeSection) ?? SECTIONS[0];
  const ActiveComponent = active.Component;

  return (
    <div className="settings-view">
      <div className="settings-view-nav">
        <div className="settings-view-nav-header">
          <h2>设置</h2>
          <button
            className="settings-view-close"
            onClick={() => setActiveView('task')}
            title="返回 (Esc)"
            aria-label="关闭设置"
            type="button"
          >
            <IconX size={16} />
          </button>
        </div>
        {SECTIONS.map((section) => (
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
        {ActiveComponent && <ActiveComponent key={active.id} />}
        {active.placeholder && (
          <PlaceholderSection
            key={active.id}
            title={active.placeholder.title}
            desc={active.placeholder.desc}
            hint={active.placeholder.hint}
            Icon={active.Icon}
          />
        )}
      </div>
    </div>
  );
}
