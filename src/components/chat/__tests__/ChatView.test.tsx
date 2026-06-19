import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { invoke } from '@tauri-apps/api/core';
import { ChatView } from '../ChatView';
import { useNavigationStore } from '../../../stores/navigationStore';
import { useAgentStore } from '../../../stores/agentStore';
import type { Project, Session, AgentInfo } from '../../../types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(() => Promise.resolve(null)) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

const project: Project = {
  id: 'p1',
  name: 'Alpha',
  description: '',
  path: 'E:/Alpha',
  tags: [],
  cover_image: null,
  open_count: 0,
  last_opened_at: null,
  starred: false,
  created_at: '2024-01-01T00:00:00.000Z',
  last_opened_tools: [],
  workspace_tools: [],
};

const agent = (t: AgentInfo['agentType']): AgentInfo => ({
  agentType: t,
  displayName: t === 'claude_code' ? 'Claude Code' : 'Codex',
  commandName: t,
  installed: true,
  path: null,
  supportsResume: true,
});

const turn = (
  id: string,
  agentType: AgentInfo['agentType'],
  startedAt: string,
  parentSessionId: string | null = null,
): Session => ({
  id,
  projectPath: 'E:/Alpha',
  agentType,
  status: 'completed',
  prompt: `prompt-${id}`,
  model: null,
  startedAt,
  finishedAt: startedAt,
  exitCode: 0,
  outputSummary: 'done',
  contextSnapshot: null,
  linkedRequirementId: null,
  parentSessionId,
  conversationId: 'c1',
});

describe('ChatView — agent-switch divider', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useNavigationStore.setState({
      activeProject: project,
      activeView: 'task',
      sidebarOpen: true,
      selectedConversationId: 'c1',
    });
    useAgentStore.setState({
      agents: [agent('claude_code'), agent('codex')],
      sessions: [
        turn('t1', 'claude_code', '2026-01-01T00:00:00Z'),
        turn('t2', 'codex', '2026-01-02T00:00:00Z', 't1'),
      ],
      conversations: [],
      loading: false,
      ptyOutput: new Map(),
      qualityReports: new Map(),
    } as Partial<ReturnType<typeof useAgentStore.getState>> as never);
  });

  it('renders a divider between turns whose agent differs', () => {
    render(<ChatView />);
    const divider = screen.queryByRole('separator');
    expect(divider).not.toBeNull();
    // The label names the transition: prior agent → current agent.
    expect(divider!.textContent).toContain('Claude Code');
    expect(divider!.textContent).toContain('Codex');
  });

  it('renders no divider when every turn uses the same agent', () => {
    useAgentStore.setState({
      agents: [agent('claude_code')],
      sessions: [
        turn('t1', 'claude_code', '2026-01-01T00:00:00Z'),
        turn('t2', 'claude_code', '2026-01-02T00:00:00Z', 't1'),
      ],
    } as Partial<ReturnType<typeof useAgentStore.getState>> as never);
    render(<ChatView />);
    expect(screen.queryByRole('separator')).toBeNull();
  });
});

describe('ChatView — edit & regenerate (A4)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Per-command mock so edit_and_regenerate + the refresh calls each resolve
    // to a shape the store can consume without throwing (refreshSessions/
    // refreshConversations spread the result — null would crash).
    const impl = async (cmd: string): Promise<unknown> => {
      if (cmd === 'get_conversation_branches') return [];
      if (cmd === 'edit_and_regenerate')
        return turn('forked', 'react_kernel', '2026-01-03T00:00:00Z', 't1');
      if (cmd === 'list_conversations') return [];
      if (cmd === 'load_sessions') return [];
      return null;
    };
    vi.mocked(invoke).mockImplementation(impl as never);
    useNavigationStore.setState({
      activeProject: project,
      activeView: 'task',
      sidebarOpen: true,
      selectedConversationId: 'c1',
    });
    useAgentStore.setState({
      agents: [agent('react_kernel')],
      sessions: [turn('t1', 'react_kernel', '2026-01-01T00:00:00Z')],
      conversations: [],
      loading: false,
      ptyOutput: new Map(),
      qualityReports: new Map(),
    } as Partial<ReturnType<typeof useAgentStore.getState>> as never);
  });

  it('edits a turn prompt and submits edit_and_regenerate with the kernel flag', async () => {
    const user = userEvent.setup();
    render(<ChatView />);

    // The per-turn edit control is present (hover-revealed via CSS opacity,
    // but present in the DOM regardless).
    const editBtn = await screen.findByRole('button', { name: '编辑并重新生成' });
    await user.click(editBtn);

    // Editing swaps the user bubble for a textarea seeded with the prompt.
    const textarea = screen.getByLabelText('编辑消息');
    expect(textarea).toHaveValue('prompt-t1');
    await user.clear(textarea);
    await user.type(textarea, '改写后的需求');

    await user.click(screen.getByRole('button', { name: '重新生成' }));

    // edit_and_regenerate fires with the edited prompt, the source turn id,
    // and kernel=true (react_kernel agent family → self-hosted ReactAgent fork).
    const calls = vi.mocked(invoke).mock.calls as unknown as [string, Record<string, unknown>][];
    const editCall = calls.find(([cmd]) => cmd === 'edit_and_regenerate');
    expect(editCall).toBeDefined();
    expect(editCall![1]).toMatchObject({
      sessionId: 't1',
      newPrompt: '改写后的需求',
      kernel: true,
    });
  });
});
