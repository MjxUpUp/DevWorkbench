import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { CommandPalette } from '../CommandPalette';
import { useNavigationStore } from '../../stores/navigationStore';
import { useProjectStore } from '../../stores/projectStore';
import { useAgentStore } from '../../stores/agentStore';
import { useKnowledgeStore } from '../../stores/knowledgeStore';
import { invoke } from '@tauri-apps/api/core';
import type { Project, KnowledgeEntry } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

const makeProject = (id: string, name: string, path: string): Project => ({
  id, name, description: '', path, tags: [], cover_image: null,
  open_count: 0, last_opened_at: null, starred: false,
  created_at: '2024-01-01T00:00:00.000Z', last_opened_tools: [], workspace_tools: [],
});

const makeKnowledge = (id: string, title: string): KnowledgeEntry =>
  ({
    id, title, content: 'c', category: 'bug', confidence: 0.9,
    project_path: null, created_at: '2024-01-01T00:00:00.000Z',
  }) as unknown as KnowledgeEntry;

describe('CommandPalette', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(invoke).mockResolvedValue([]);
    useNavigationStore.setState({
      commandPaletteOpen: true, activeView: 'task', activeProject: null,
      selectedSessionId: null,
    });
    useProjectStore.setState({ projects: [], loading: false, error: null });
    useAgentStore.setState({ sessions: [] } as Partial<ReturnType<typeof useAgentStore.getState>> as never);
    useKnowledgeStore.setState({ searchResults: [], loading: false, entries: [] });
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

  it('renders knowledge hits as non-actionable rows (no button role)', async () => {
    // Knowledge entries only surface after a backend search keyed off the query.
    useKnowledgeStore.setState({ searchResults: [makeKnowledge('k1', 'crash fix')] });
    const user = userEvent.setup();
    render(<CommandPalette />);

    await user.type(screen.getByPlaceholderText(/搜索/), 'crash');

    expect(screen.getByText('crash fix')).toBeInTheDocument();
    expect(screen.getByText(/置信度 90%/)).toBeInTheDocument();
    // Knowledge has no deep link — it must render as a static row, not a button.
    expect(screen.queryByRole('button', { name: /crash fix/ })).toBeNull();
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
});
