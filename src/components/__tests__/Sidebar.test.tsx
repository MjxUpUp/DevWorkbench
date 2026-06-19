import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Sidebar } from '../Sidebar';
import { ToastProvider } from '../Toast';
import { useNavigationStore } from '../../stores/navigationStore';
import { useProjectStore } from '../../stores/projectStore';
import { useAgentStore } from '../../stores/agentStore';
import { invoke } from '@tauri-apps/api/core';
import type { Project, Conversation } from '../../types';

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

const makeConversation = (id: string, title: string, projectPath: string, over: Partial<Conversation> = {}): Conversation => ({
  id,
  projectPath,
  title,
  lastAgent: 'claude_code',
  status: 'active',
  startedAt: '2026-01-01T00:00:00.000Z',
  lastActivityAt: '2026-01-02T00:00:00.000Z',
  pinned: false,
  ...over,
});

describe('Sidebar', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.mocked(invoke).mockResolvedValue([]);
    useNavigationStore.setState({
      activeProject: null,
      activeView: 'task',
      sidebarOpen: true,
      selectedConversationId: null,
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
    render(<ToastProvider><Sidebar /></ToastProvider>);
    expect(screen.getByRole('button', { name: '移除 Alpha' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '移除 Beta' })).toBeInTheDocument();
  });

  it('removes a project via remove_project and does not select it (stopPropagation)', async () => {
    const user = userEvent.setup();
    useProjectStore.setState({
      projects: [makeProject('p1', 'Alpha', 'E:/Alpha')],
    });
    render(<ToastProvider><Sidebar /></ToastProvider>);

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
    render(<ToastProvider><Sidebar /></ToastProvider>);

    await user.click(screen.getByRole('button', { name: '搜索' }));

    // The search entry pops the centered palette modal rather than switching views.
    expect(useNavigationStore.getState().commandPaletteOpen).toBe(true);
    expect(useNavigationStore.getState().activeView).toBe('task');
  });

  it('lists the active project\'s conversations and selects one on click', async () => {
    const user = userEvent.setup();
    const proj = makeProject('p1', 'Alpha', 'E:/Alpha');
    useProjectStore.setState({ projects: [proj], loading: false, error: null });
    useNavigationStore.setState({
      activeProject: proj,
      activeView: 'task',
      sidebarOpen: true,
      selectedConversationId: null,
    });
    // Seed the store: two conversations under Alpha, newest-activity first is
    // enforced by getConversationsForProject (we feed them pre-sorted so the
    // selector's sort is a no-op here).
    useAgentStore.setState({
      conversations: [
        makeConversation('c-new', 'newer topic', 'E:/Alpha', { lastActivityAt: '2026-02-01T00:00:00.000Z' }),
        makeConversation('c-old', 'older topic', 'E:/Alpha', { lastActivityAt: '2026-01-01T00:00:00.000Z' }),
      ],
    } as Partial<ReturnType<typeof useAgentStore.getState>> as never);

    render(<ToastProvider><Sidebar /></ToastProvider>);

    // Both conversations render under the active project.
    expect(screen.getByText('newer topic')).toBeInTheDocument();
    expect(screen.getByText('older topic')).toBeInTheDocument();

    // Clicking selects it in navigation state.
    await user.click(screen.getByText('newer topic'));
    expect(useNavigationStore.getState().selectedConversationId).toBe('c-new');
  });

  it('archives a conversation via the 📦 action button', async () => {
    const user = userEvent.setup();
    const proj = makeProject('p1', 'Alpha', 'E:/Alpha');
    useProjectStore.setState({ projects: [proj], loading: false, error: null });
    useNavigationStore.setState({
      activeProject: proj,
      activeView: 'task',
      sidebarOpen: true,
      selectedConversationId: null,
    });
    useAgentStore.setState({
      conversations: [makeConversation('c1', 'topic', 'E:/Alpha')],
    } as Partial<ReturnType<typeof useAgentStore.getState>> as never);

    render(<ToastProvider><Sidebar /></ToastProvider>);

    await user.click(screen.getByRole('button', { name: '归档' }));
    expect(invoke).toHaveBeenCalledWith('archive_conversation', { id: 'c1' });
  });

  it('deletes a conversation and the undo toast restores it', async () => {
    const user = userEvent.setup();
    const proj = makeProject('p1', 'Alpha', 'E:/Alpha');
    useProjectStore.setState({ projects: [proj], loading: false, error: null });
    useNavigationStore.setState({
      activeProject: proj,
      activeView: 'task',
      sidebarOpen: true,
      selectedConversationId: null,
    });
    useAgentStore.setState({
      conversations: [makeConversation('c1', 'topic', 'E:/Alpha')],
    } as Partial<ReturnType<typeof useAgentStore.getState>> as never);

    render(<ToastProvider><Sidebar /></ToastProvider>);

    await user.click(screen.getByRole('button', { name: '删除' }));
    expect(invoke).toHaveBeenCalledWith('delete_conversation', { id: 'c1' });

    // The undo toast surfaces a 撤销 action; clicking it restores the row.
    const undo = await screen.findByRole('button', { name: '撤销' });
    await user.click(undo);
    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith('restore_conversation', { id: 'c1' }),
    );
  });

  it('does not flash the empty state while the first conversation refresh is in flight', () => {
    // Regression for the "加载闪动" symptom: on first project click, refresh
    // is still in flight and conversations is empty. Rendering the empty state
    // there flashes it for a frame before the list pops in. ConversationList
    // must render NOTHING until the first refresh resolves, then decide.
    const proj = makeProject('p1', 'Alpha', 'E:/Alpha');
    useProjectStore.setState({ projects: [proj], loading: false, error: null });
    useNavigationStore.setState({
      activeProject: proj,
      activeView: 'task',
      sidebarOpen: true,
      selectedConversationId: null,
    });
    // Pin refresh to a never-resolving promise so loadedPaths is never marked —
    // this is exactly the in-flight window where the flash used to occur.
    useAgentStore.setState({
      conversations: [],
      refreshConversations: () => new Promise<void>(() => {}),
    } as Partial<ReturnType<typeof useAgentStore.getState>> as never);

    render(<ToastProvider><Sidebar /></ToastProvider>);

    // No empty-state text while the first load is still pending.
    expect(screen.queryByText('暂无对话')).toBeNull();
  });

  it('shows the empty state once the first refresh resolves with zero conversations', async () => {
    // After the first refresh resolves and the project genuinely has zero
    // conversations, the empty state is legitimate and must appear.
    const proj = makeProject('p1', 'Alpha', 'E:/Alpha');
    useProjectStore.setState({ projects: [proj], loading: false, error: null });
    useNavigationStore.setState({
      activeProject: proj,
      activeView: 'task',
      sidebarOpen: true,
      selectedConversationId: null,
    });
    // Resolve immediately so loadedPaths is marked on the first effect tick.
    useAgentStore.setState({
      conversations: [],
      refreshConversations: () => Promise.resolve(),
    } as Partial<ReturnType<typeof useAgentStore.getState>> as never);

    render(<ToastProvider><Sidebar /></ToastProvider>);

    expect(await screen.findByText('暂无对话')).toBeInTheDocument();
  });
});
