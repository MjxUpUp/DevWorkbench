import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SettingsView } from '../SettingsView';
import { useNavigationStore } from '../../../stores/navigationStore';
import { useSkillsStore } from '../../../stores/skillsStore';

/**
 * SettingsView owns the section nav + routing. This pins the A7 rename:
 * the section formerly id'd "plugins" (label "能力总览") is now "capability",
 * backed by CapabilitySection. The nav must still surface it, clicking it must
 * route to CapabilitySection (which renders the built-in tools overview), and
 * nothing should reference the dead PluginsSection / DashboardView.
 */
const mockInvoke = vi.hoisted(() => vi.fn());
const toastSpies = vi.hoisted(() => ({
  success: vi.fn(), error: vi.fn(), info: vi.fn(), warning: vi.fn(),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));
vi.mock('../../Toast', () => ({ useToast: () => toastSpies }));

function setupInvoke() {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'load_settings') return Promise.resolve({});
    if (cmd === 'list_skills') return Promise.resolve([]);
    if (cmd === 'skill_catalog') return Promise.resolve([]);
    if (cmd === 'mcp_catalog') return Promise.resolve([]);
    return Promise.resolve(undefined);
  });
}

describe('SettingsView (A7 plugins→capability rename)', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    setupInvoke();
    // 每个用例从干净基线开始——navigationStore/skillsStore 是模块级单例，上一用例
    // 残留的 settingsInitialSection 会污染默认分区断言（直达 skills 后未重置则默认用例假绿）。
    useNavigationStore.setState({ settingsInitialSection: null });
    useSkillsStore.setState({ installed: [], catalog: [], loading: false });
  });

  it('renders the 能力总览 nav entry', () => {
    render(<SettingsView />);
    expect(screen.getByRole('button', { name: '能力总览' })).toBeInTheDocument();
  });

  it('routes to CapabilitySection on click — built-in tools overview appears', async () => {
    const user = userEvent.setup();
    render(<SettingsView />);
    await user.click(screen.getByRole('button', { name: '能力总览' }));
    // CapabilitySection renders the built-in tools group + the dispatch_subagent
    // entry that proves it is the renamed component, not a leftover.
    expect((await screen.findAllByText('内置工具')).length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('dispatch_subagent', { exact: false })).toBeInTheDocument();
  });

  it('defaults to the agent-tools section (not capability)', () => {
    render(<SettingsView />);
    // The capability overview is NOT the default landing section.
    expect(screen.queryByText('内置工具')).not.toBeInTheDocument();
  });

  it('lands on the skills section when entered via settingsInitialSection', () => {
    // 命令面板「技能」会先把 settingsInitialSection 置为 'skills'，SettingsView 应消费它
    // 直达技能分区（技能目录统一归设置页管理的入口路径），而非默认的 agent-tools。
    useNavigationStore.setState({ settingsInitialSection: 'skills' });
    render(<SettingsView />);
    expect(screen.getByText('技能管理')).toBeInTheDocument();
    // 消费后随即清空，避免下次从用户菜单进设置仍落在技能分区。
    expect(useNavigationStore.getState().settingsInitialSection).toBeNull();
  });

  it('clears settingsInitialSection after consuming so a later normal entry defaults back', () => {
    // 锁死「消费即清空」语义：直达 skills 一次后，再次正常进入设置不应残留 skills 分区。
    useNavigationStore.setState({ settingsInitialSection: 'skills' });
    const { unmount } = render(<SettingsView />);
    expect(screen.getByText('技能管理')).toBeInTheDocument();
    expect(useNavigationStore.getState().settingsInitialSection).toBeNull();
    unmount();

    // 第二次进入：用户菜单正常进设置（无直达意图）→ 应回默认 agent-tools。
    render(<SettingsView />);
    expect(screen.queryByText('技能管理')).not.toBeInTheDocument();
  });
});
