import { useState, useEffect } from 'react';
import { AppearanceSection } from './AppearanceSection';
import { AgentSection } from './AgentSection';
import { ProvidersSection } from './ProvidersSection';
import { McpSection } from './McpSection';
import { MemorySection } from './MemorySection';
import { SkillsSection } from './SkillsSection';
import { UsageStatsSection } from './UsageStatsSection';
import { PlaceholderSection } from './PlaceholderSection';
import { CapabilitySection } from './CapabilitySection';
import { CommandsSection } from './CommandsSection';
import { HooksSection } from './HooksSection';
import { SubAgentsSection } from './SubAgentsSection';
import { TraceSection } from './TraceSection';
import type { SettingsSection } from './types';
import { useSettingsStore } from '../../stores/settingsStore';
import {
  IconSun, IconTerminal, IconCpu,
  IconSparkles, IconBrain, IconInbox,
  IconUser, IconPlay, IconEdit, IconStar,
  IconDashboard, IconCode, IconChat, IconX,
} from '../Icons';
import { useNavigationStore } from '../../stores/navigationStore';

type SettingsIcon = React.FC<{ size?: number; className?: string }>;

/**
 * Settings categories. Slimmed down from the earlier 15-section list:
 *   - removed "AI 配置" (was GeneralSection — didn't match the label's intent)
 *   - removed "代码预览" (placeholder)
 *   - removed "索引" (placeholder)
 * Remaining sections map to a real component or share the PlaceholderSection
 * aesthetic. The SettingsSection id union lives in ./types — shared with
 * navigationStore so the command palette's deep-link target is type-checked
 * (a typo'd section id fails at compile time instead of silently falling back
 * to the default section at runtime).
 */

interface SectionDef {
  id: SettingsSection;
  label: string;
  Icon: SettingsIcon;
  Component?: React.FC;
  placeholder?: { title: string; desc: string; hint?: string };
  /** 语义分组（v3：14 tab 按业务聚类，不再平铺）。 */
  group: '智能体' | '能力扩展' | '记忆与输出' | '数据与诊断' | '入门';
}

const SECTIONS: SectionDef[] = [
  // 智能体
  { id: 'agent-tools', label: '智能体工具', Icon: IconTerminal, Component: AgentSection, group: '智能体' },
  { id: 'providers', label: '模型供应商', Icon: IconCpu, Component: ProvidersSection, group: '智能体' },
  { id: 'capability', label: '能力总览', Icon: IconInbox, Component: CapabilitySection, group: '智能体' },
  { id: 'sub-agents', label: '子智能体', Icon: IconUser, Component: SubAgentsSection, group: '智能体' },
  // 能力扩展
  { id: 'skills', label: '技能', Icon: IconSparkles, Component: SkillsSection, group: '能力扩展' },
  { id: 'mcp', label: 'MCP 服务器', Icon: IconBrain, Component: McpSection, group: '能力扩展' },
  { id: 'commands', label: '命令', Icon: IconPlay, Component: CommandsSection, group: '能力扩展' },
  { id: 'hooks', label: '钩子', Icon: IconEdit, Component: HooksSection, group: '能力扩展' },
  // 记忆与输出
  { id: 'memory', label: '记忆', Icon: IconStar, Component: MemorySection, group: '记忆与输出' },
  { id: 'output-style', label: '输出样式', Icon: IconSun, Component: AppearanceSection, group: '记忆与输出' },
  // 数据与诊断
  { id: 'usage-stats', label: '使用统计', Icon: IconDashboard, Component: UsageStatsSection, group: '数据与诊断' },
  { id: 'trace', label: 'LLM 追踪', Icon: IconCode, Component: TraceSection, group: '数据与诊断' },
  // 入门
  { id: 'onboarding', label: '引导', Icon: IconChat, placeholder: { title: '引导', desc: '新手引导与帮助文档', hint: '引导功能正在开发中，敬请期待' }, group: '入门' },
];

const GROUP_ORDER: SectionDef['group'][] = ['智能体', '能力扩展', '记忆与输出', '数据与诊断', '入门'];

export function SettingsView() {
  // 外部入口（命令面板「技能」）可指定进设置页时直达的分区：initializer 消费一次，
  // 下方的 useEffect 随即清空，避免下次从用户菜单进设置仍落在该分区。
  const setSettingsInitialSection = useNavigationStore((s) => s.setSettingsInitialSection);
  const [activeSection, setActiveSection] = useState<SettingsSection>(() => {
    const initial = useNavigationStore.getState().settingsInitialSection;
    if (initial) {
      const match = SECTIONS.find((s) => s.id === initial);
      if (match) return match.id;
    }
    return 'agent-tools';
  });
  const loadSettings = useSettingsStore((s) => s.loadSettings);
  const setActiveView = useNavigationStore((s) => s.setActiveView);

  // Load settings once on mount — shared store eliminates per-section loading
  useEffect(() => { loadSettings(); }, [loadSettings]);

  // 消费外部入口指定的直达分区后清空，防止污染下次正常进入设置。
  useEffect(() => {
    if (useNavigationStore.getState().settingsInitialSection !== null) {
      setSettingsInitialSection(null);
    }
  }, [setSettingsInitialSection]);

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
        {GROUP_ORDER.map((groupName) => {
          const groupSections = SECTIONS.filter((s) => s.group === groupName);
          if (groupSections.length === 0) return null;
          return (
            <div key={groupName} className="settings-nav-group">
              <div className="settings-nav-group-label">{groupName}</div>
              {groupSections.map((section) => (
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
          );
        })}
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
