import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Sidebar } from '../Sidebar';
import { useNavigationStore } from '../../stores/navigationStore';
import { useProjectStore } from '../../stores/projectStore';
import { invoke } from '@tauri-apps/api/core';
import type { Project } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

const makeProject = (id: string, name: string, path: string): Project => ({
  id,
  name,
  description: '',
  path,
  tags: [],
  cover_image: null,
  open_count: 0,
  last_opened_at: null,
  starred: false,
  created_at: '2024-01-01T00:00:00.000Z',
  last_opened_tools: [],
  workspace_tools: [],
});

describe('Sidebar', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(invoke).mockResolvedValue([]);
    useNavigationStore.setState({
      activeProject: null,
      activeView: 'task',
      sidebarOpen: true,
      selectedSessionId: null,
    });
    useProjectStore.setState({
      projects: [],
      loading: false,
      error: null,
    });
  });

  it('does not render a duplicate brand/logo mark (handled by TitleBar)', () => {
    const { container } = render(<Sidebar />);
    expect(container.querySelector('.left-column-logo')).toBeNull();
    // The literal "Z" logo text must not appear anywhere in the sidebar.
    expect(screen.queryByText('Z')).toBeNull();
  });

  it('renders each project with an accessible remove control', () => {
    useProjectStore.setState({
      projects: [makeProject('p1', 'Alpha', 'E:/Alpha'), makeProject('p2', 'Beta', 'E:/Beta')],
    });
    render(<Sidebar />);
    expect(screen.getByRole('button', { name: '移除 Alpha' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '移除 Beta' })).toBeInTheDocument();
  });

  it('removes a project via remove_project and does not select it (stopPropagation)', async () => {
    const user = userEvent.setup();
    useProjectStore.setState({
      projects: [makeProject('p1', 'Alpha', 'E:/Alpha')],
    });
    render(<Sidebar />);

    await user.click(screen.getByRole('button', { name: '移除 Alpha' }));

    expect(invoke).toHaveBeenCalledWith('remove_project', { id: 'p1' });
    // The row's onClick (handleSelectProject → selectProject + setActiveView) must
    // not fire when the remove button is clicked. activeProject started null and
    // must stay null — proving stopPropagation worked.
    expect(useNavigationStore.getState().activeProject).toBeNull();
  });

  it('opens the command-palette modal (not a view) when the 搜索 button is clicked', async () => {
    const user = userEvent.setup();
    useNavigationStore.setState({ commandPaletteOpen: false, activeView: 'task' });
    render(<Sidebar />);

    await user.click(screen.getByRole('button', { name: '搜索' }));

    // The search entry pops the centered palette modal rather than switching views.
    expect(useNavigationStore.getState().commandPaletteOpen).toBe(true);
    expect(useNavigationStore.getState().activeView).toBe('task');
  });
});
