import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, act } from '@testing-library/react';
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
  displayName: t === 'claude_code' ? 'Claude Code' : 'Codex',
  commandName: t,
  installed: true,
  path: null,
  supportsResume: true,
});

const turn = (
  id: string,
  startedAt: string,
  parentSessionId: string | null = null,
): Session => ({
  id,
  projectPath: 'E:/Alpha',
  agentType: 'claude_code',
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

function countBranchCalls(): number {
  return vi.mocked(invoke).mock.calls.filter(
    ([cmd]) => cmd === 'get_conversation_branches',
  ).length;
}

describe('ChatView — branches effect (F10: no per-token refetch)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Default mock: most commands return null/empty. CRITICAL: get_quality_report
    // must return null (not []) — an array is truthy and would be stored as a
    // malformed QualityReport, crashing AgentMessage on `.checks.filter`.
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'get_quality_report_for_session') return null;
      if (cmd === 'load_sessions') return [];
      if (cmd === 'list_conversations') return [];
      return [];
    });
    useNavigationStore.setState({
      activeProject: project,
      activeView: 'task',
      selectedConversationId: 'c1',
    });
    useAgentStore.setState({
      agents: [agent('claude_code')],
      sessions: [turn('t1', '2026-01-01T00:00:00Z')],
      conversations: [],
      loading: false,
      ptyOutput: new Map(),
      qualityReports: new Map(),
    } as Partial<ReturnType<typeof useAgentStore.getState>> as never);
  });

  it('does NOT refetch branches when allSessions identity changes but turns do not', async () => {
    // First render pulls branches once for the active conversation.
    const { unmount } = render(<ChatView />);
    // Flush the async get_conversation_branches promise.
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    const firstCount = countBranchCalls();
    expect(firstCount).toBe(1);

    // Simulate streaming: refreshSessions swaps the sessions array reference on
    // every token (new array, same turns). Previously this was in the deps and
    // re-fired the effect per token.
    for (let i = 0; i < 5; i++) {
      const prev = useAgentStore.getState().sessions;
      // New array reference, SAME turn (no new turn landed) — this is exactly
      // what refreshSessions does during streaming.
      useAgentStore.setState({ sessions: [...prev] } as never);
      await act(async () => { await Promise.resolve(); });
    }

    // No additional branch fetches: turns.length didn't change.
    expect(countBranchCalls()).toBe(1);
    unmount();
  });

  it('refetches branches when turns.length changes (new turn landed)', async () => {
    const { unmount } = render(<ChatView />);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    expect(countBranchCalls()).toBe(1);

    // A new turn lands (length goes 1 → 2): turns.length changes → refetch.
    useAgentStore.setState({
      sessions: [
        turn('t1', '2026-01-01T00:00:00Z'),
        turn('t2', '2026-01-02T00:00:00Z', 't1'),
      ],
    } as never);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });

    expect(countBranchCalls()).toBe(2);
    unmount();
  });

  it('refetches branches when activeConversationId changes', async () => {
    const { unmount } = render(<ChatView />);
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });
    expect(countBranchCalls()).toBe(1);

    useNavigationStore.setState({ selectedConversationId: 'c2' });
    await act(async () => { await Promise.resolve(); await Promise.resolve(); });

    expect(countBranchCalls()).toBe(2);
    unmount();
  });
});
