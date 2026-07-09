import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { CommandPalette } from '../CommandPalette';
import { useNavigationStore } from '../../stores/navigationStore';
import { useProjectStore } from '../../stores/projectStore';
import { useAgentStore } from '../../stores/agentStore';
import { invoke } from '@tauri-apps/api/core';
import type { Project } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

const makeProject = (id: string, name: string, path: string): Project => ({
  id, name, description: '', path, tags: [], cover_image: null,
  open_count: 0, last_opened_at: null, starred: false,
  created_at: '2024-01-01T00:00:00.000Z', last_opened_tools: [], workspace_tools: [],
});

describe('CommandPalette', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(invoke).mockResolvedValue([]);
    useNavigationStore.setState({
      commandPaletteOpen: true, activeView: 'task', activeProject: null,
      selectedConversationId: null,
    });
    useProjectStore.setState({ projects: [], loading: false, error: null });
    useAgentStore.setState({ sessions: [] } as Partial<ReturnType<typeof useAgentStore.getState>> as never);
  });

  it('renders nothing when closed', () => {
    useNavigationStore.setState({ commandPaletteOpen: false });
    const { container } = render(<CommandPalette />);
    expect(container.querySelector('.command-palette-overlay')).toBeNull();
  });

  it('shows the command group (操作) with navigation actions when open and empty', () => {
    render(<CommandPalette />);
    expect(screen.getByText('操作')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /创建任务/ })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '技能' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '设置' })).toBeInTheDocument();
  });

  it('picks a project → selects it, switches to task view, and closes the modal', async () => {
    const user = userEvent.setup();
    useProjectStore.setState({ projects: [makeProject('p1', 'Alpha', 'E:/Alpha')] });
    render(<CommandPalette />);

    await user.type(screen.getByPlaceholderText(/搜索/), 'alpha');
    await user.click(screen.getByRole('button', { name: /Alpha/ }));

    const nav = useNavigationStore.getState();
    expect(nav.activeProject?.id).toBe('p1');
    expect(nav.activeView).toBe('task');
    expect(nav.commandPaletteOpen).toBe(false);
  });

  it('closes on Escape', async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);
    expect(useNavigationStore.getState().commandPaletteOpen).toBe(true);

    await user.keyboard('{Escape}');

    expect(useNavigationStore.getState().commandPaletteOpen).toBe(false);
  });

  it('Enter triggers the first actionable result', async () => {
    const user = userEvent.setup();
    render(<CommandPalette />);

    await user.type(screen.getByPlaceholderText(/搜索/), '设置');
    await user.keyboard('{Enter}');

    expect(useNavigationStore.getState().activeView).toBe('settings');
    expect(useNavigationStore.getState().commandPaletteOpen).toBe(false);
  });

  it('技能 command routes to settings and targets the skills section', async () => {
    // 技能目录已下放设置页统一管理——命令面板的「技能」直达设置页技能分区，
    // 不再切到旧的主页 skills view。
    const user = userEvent.setup();
    render(<CommandPalette />);
    await user.click(screen.getByRole('button', { name: '技能' }));
    const nav = useNavigationStore.getState();
    expect(nav.activeView).toBe('settings');
    expect(nav.settingsInitialSection).toBe('skills');
    expect(nav.commandPaletteOpen).toBe(false);
  });
});
