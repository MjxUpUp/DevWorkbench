import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
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
  parentSessionId: null,
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
        turn('t2', 'codex', '2026-01-02T00:00:00Z'),
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
        turn('t2', 'claude_code', '2026-01-02T00:00:00Z'),
      ],
    } as Partial<ReturnType<typeof useAgentStore.getState>> as never);
    render(<ChatView />);
    expect(screen.queryByRole('separator')).toBeNull();
  });
});
