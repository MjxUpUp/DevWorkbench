import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { invoke } from '@tauri-apps/api/core';
import { ChatView } from '../ChatView';
import { useNavigationStore } from '../../../stores/navigationStore';
import { useAgentStore } from '../../../stores/agentStore';
import type { Project, Session, AgentInfo } from '../../../types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(() => Promise.resolve(null)) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));
vi.mock('../../Toast', () => ({
  useToast: () => ({ success: vi.fn(), error: vi.fn(), info: vi.fn(), toast: vi.fn() }),
}));

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
  displayName: 'Claude Code',
  commandName: t,
  installed: true,
  path: null,
  supportsResume: true,
});

function makeSession(id: string, status: Session['status']): Session {
  return {
    id,
    projectPath: 'E:/Alpha',
    agentType: 'claude_code',
    status,
    prompt: `prompt-${id}`,
    model: null,
    startedAt: '2026-01-01T00:00:00Z',
    finishedAt: '2026-01-01T00:00:00Z',
    exitCode: 0,
    outputSummary: '',
    contextSnapshot: null,
    linkedRequirementId: null,
    parentSessionId: null,
    conversationId: 'new-conv',
  };
}

describe('ChatView — handleSend re-entry guard (F12: closure race)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useNavigationStore.setState({
      activeProject: project,
      activeView: 'task',
      sidebarOpen: true,
      // null = first send in a fresh conversation → goes through createConversation
      selectedConversationId: null,
    });
    useAgentStore.setState({
      agents: [agent('claude_code')],
      sessions: [],
      conversations: [],
      loading: false,
      ptyOutput: new Map(),
      qualityReports: new Map(),
    } as Partial<ReturnType<typeof useAgentStore.getState>> as never);
  });

  it('a second rapid click during the await does NOT spawn a second turn', async () => {
    // Block spawn_agent_session until we release it, so the second click lands
    // while the first is still in flight (the race window the guard must close).
    let releaseSpawn!: () => void;
    const spawnPromise = new Promise((resolve) => {
      releaseSpawn = () => resolve(makeSession('s-running', 'running'));
    });

    const impl = async (cmd: string): Promise<unknown> => {
      if (cmd === 'spawn_agent_session') {
        // Simulate the store flipping runningSession to non-empty the moment
        // spawn_agent_session fires — this is what would normally update the
        // guard, but the closure captured the OLD null on the second click.
        useAgentStore.setState({
          sessions: [makeSession('s-running', 'running')],
        } as never);
        return spawnPromise;
      }
      if (cmd === 'get_conversation_branches') return [];
      if (cmd === 'list_conversations') return [];
      if (cmd === 'load_sessions') return [makeSession('s-running', 'running')];
      if (cmd === 'get_quality_report_for_session') return null;
      if (cmd === 'recommend_agent_for_project') return null;
      return null;
    };
    vi.mocked(invoke).mockImplementation(impl as never);

    const user = userEvent.setup();
    render(<ChatView />);

    // Land the empty-state composer (project set, no turns → first-turn surface).
    const textarea = await screen.findByTestId('chat-composer-input');
    await user.type(textarea, '做点东西');

    // Wait for the auto-recommend-agent effect to settle (selectedAgent starts
    // null and is set asynchronously via recommendAgent → setSelectedAgent).
    // Until it resolves, canSend is false and the send button stays disabled.
    const sendBtn = await screen.findByTestId('composer-send-btn');
    await waitFor(() => expect(sendBtn).not.toBeDisabled());

    // Two clicks in quick succession, before the first await resolves.
    await user.click(sendBtn);
    await user.click(sendBtn);

    // Release the first spawn_agent_session; let the post-send flush settle.
    await act(async () => {
      releaseSpawn();
      await Promise.resolve();
      await Promise.resolve();
    });

    // Exactly ONE spawn_agent_session call — the second click was rejected by
    // the ref guard because the first was still in flight. Without the guard,
    // the closure still saw runningSession=null and BOTH clicks would have
    // fired spawn → two turns.
    const spawnCalls = vi.mocked(invoke).mock.calls.filter(
      ([cmd]) => cmd === 'spawn_agent_session',
    );
    expect(spawnCalls).toHaveLength(1);
  });
});
