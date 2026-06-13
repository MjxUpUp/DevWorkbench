import { useState, useEffect } from 'react';
import { GeneralSection } from './GeneralSection';
import { AppearanceSection } from './AppearanceSection';
import { AgentSection } from './AgentSection';
import { ProvidersSection } from './ProvidersSection';
import { McpSection } from './McpSection';
import { SkillsSection } from './SkillsSection';
import { PlaceholderSection } from './PlaceholderSection';
import { useSettingsStore } from '../../stores/settingsStore';
import {
  IconSettings, IconSun, IconTerminal, IconCpu,
  IconSparkles, IconBrain, IconCode, IconInbox,
  IconUser, IconPlay, IconEdit, IconStar,
  IconFolderOpen, IconDashboard, IconChat,
} from '../Icons';

type SettingsIcon = React.FC<{ size?: number; className?: string }>;

/**
 * Settings categories aligned to zcode's 15-section navigation order.
 * Implemented categories map to a real section component; pending ones reuse
 * PlaceholderSection so the whole nav shares one aesthetic.
 *
 * zcode order: AI配置 / 代码预览 / 智能体工具 / 模型供应商 / 插件 / 技能 /
 *              MCP服务器 / 子智能体 / 命令 / 钩子 / 记忆 / 索引 / 输出样式 / 使用统计 / 引导
 */
export type SettingsSection =
  | 'ai-config' | 'code-preview' | 'agent-tools' | 'providers' | 'plugins'
  | 'skills' | 'mcp' | 'sub-agents' | 'commands' | 'hooks'
  | 'memory' | 'indexing' | 'output-style' | 'usage-stats' | 'onboarding';

interface SectionDef {
  id: SettingsSection;
  label: string;
  Icon: SettingsIcon;
  Component?: React.FC;
  placeholder?: { title: string; desc: string; hint?: string };
}

const SECTIONS: SectionDef[] = [
  { id: 'ai-config', label: 'AI 配置', Icon: IconSettings, Component: GeneralSection },
  { id: 'code-preview', label: '代码预览', Icon: IconCode, placeholder: { title: '代码预览', desc: '配置代码差异预览与语法高亮样式', hint: '代码预览功能正在开发中，敬请期待' } },
  { id: 'agent-tools', label: '智能体工具', Icon: IconTerminal, Component: AgentSection },
  { id: 'providers', label: '模型供应商', Icon: IconCpu, Component: ProvidersSection },
  { id: 'plugins', label: '插件', Icon: IconInbox, placeholder: { title: '插件', desc: '管理已安装的插件与扩展能力', hint: '插件管理功能正在开发中，敬请期待' } },
  { id: 'skills', label: '技能', Icon: IconSparkles, Component: SkillsSection },
  { id: 'mcp', label: 'MCP 服务器', Icon: IconBrain, Component: McpSection },
  { id: 'sub-agents', label: '子智能体', Icon: IconUser, placeholder: { title: '子智能体', desc: '配置可被主智能体调用的子智能体', hint: '子智能体配置正在开发中，敬请期待' } },
  { id: 'commands', label: '命令', Icon: IconPlay, placeholder: { title: '命令', desc: '管理自定义斜杠命令', hint: '命令管理功能正在开发中，敬请期待' } },
  { id: 'hooks', label: '钩子', Icon: IconEdit, placeholder: { title: '钩子', desc: '配置生命周期钩子与事件回调', hint: '钩子配置正在开发中，敬请期待' } },
  { id: 'memory', label: '记忆', Icon: IconStar, placeholder: { title: '记忆', desc: '管理智能体长期记忆条目', hint: '记忆管理功能正在开发中，敬请期待' } },
  { id: 'indexing', label: '索引', Icon: IconFolderOpen, placeholder: { title: '索引', desc: '代码库索引与语义检索配置', hint: '索引功能正在开发中，敬请期待' } },
  { id: 'output-style', label: '输出样式', Icon: IconSun, Component: AppearanceSection },
  { id: 'usage-stats', label: '使用统计', Icon: IconDashboard, placeholder: { title: '使用统计', desc: '查看用量、成本与调用统计', hint: '使用统计功能正在开发中，敬请期待' } },
  { id: 'onboarding', label: '引导', Icon: IconChat, placeholder: { title: '引导', desc: '新手引导与帮助文档', hint: '引导功能正在开发中，敬请期待' } },
];

export function SettingsView() {
  const [activeSection, setActiveSection] = useState<SettingsSection>('ai-config');
  const loadSettings = useSettingsStore((s) => s.loadSettings);

  // Load settings once on mount — shared store eliminates per-section loading
  useEffect(() => { loadSettings(); }, [loadSettings]);

  const active = SECTIONS.find(s => s.id === activeSection) ?? SECTIONS[0];
  const ActiveComponent = active.Component;

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
