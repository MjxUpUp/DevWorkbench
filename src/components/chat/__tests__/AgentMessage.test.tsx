import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { AgentMessage } from '../AgentMessage';
import { useAgentStore } from '../../../stores/agentStore';
import type { Session, QualityReport } from '../../../types';

// AgentMessage no longer invokes any IPC command in its own effect (the
// raw-output fetch path was removed with the CLI retirement — ReactKernel
// always renders BlocksView). invoke is still mocked so accidental calls
// surface as assertion failures rather than unhandled rejections.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

const base: Session = {
  id: 's1',
  projectPath: '/p',
  agentType: 'react_kernel',
  status: 'completed',
  prompt: 'fix the bug',
  model: null,
  startedAt: '2026-01-01T00:00:00Z',
  finishedAt: '2026-01-01T00:01:00Z',
  exitCode: 0,
  outputSummary: '...truncated tail',
  contextSnapshot: null,
  linkedRequirementId: null,
  parentSessionId: null,
  conversationId: null,
};

const noReport: QualityReport | null = null;

describe('AgentMessage — structured blocks rendering', () => {
  // ReactKernel is the sole agent now; every session renders BlocksView in
  // every state (running-empty → waiting hint, running accumulating, completed
  // → persisted blocks). The terminal/markdown raw path is gone.
  beforeEach(() => {
    vi.clearAllMocks();
    useAgentStore.setState({ sessionBlocks: new Map() } as never);
  });

  it('structured agent running shows chat-blocks waiting, NOT a full-output fetch', () => {
    // react_kernel while running with no block yet (e.g. the model gateway
    // holding its response) renders BlocksView's "等待模型响应" waiting state.
    // No IPC call is made — the raw-output fetch path was deleted.
    const runningSession: Session = { ...base, status: 'running', outputSummary: null, finishedAt: null, exitCode: null };
    render(<AgentMessage session={runningSession} running={true} qualityReport={noReport} />);

    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('read_session_output_cmd', expect.anything());
    expect(screen.getByText('等待模型响应')).toBeInTheDocument();
  });

  it('renders BlocksView when structured blocks are in store (chat-blocks path)', () => {
    // The accumulated live blocks drive BlocksView, NOT any terminal/markdown
    // path. And because blocks are authoritative here, no output fetch happens.
    useAgentStore.setState({
      sessionBlocks: new Map([
        ['s1', [
          { kind: 'text', content: 'block reply' },
          { kind: 'tool_use', name: 'Read', input: { file_path: '/x' } },
        ]],
      ]),
    } as never);

    render(<AgentMessage session={base} running={false} qualityReport={noReport} />);

    expect(screen.getByText('block reply')).toBeInTheDocument();
    expect(screen.getByText('Read')).toBeInTheDocument();
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('read_session_output_cmd', expect.anything());
  });

  it('replays persisted session.blocks when the live Map is empty (history reload)', () => {
    // After a reload or project-switch, the live in-memory Map is empty but the
    // completed session carries its persisted blocks from the DB. Those must
    // drive BlocksView.
    useAgentStore.setState({ sessionBlocks: new Map() } as never);

    render(
      <AgentMessage
        session={{ ...base, blocks: [{ kind: 'text', content: 'persisted reply' }] }}
        running={false}
        qualityReport={noReport}
      />
    );

    expect(screen.getByText('persisted reply')).toBeInTheDocument();
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('read_session_output_cmd', expect.anything());
  });

  it('prefers live blocks over persisted session.blocks (running session shadowing)', () => {
    // While a session runs, the live Map is authoritative even if a stale
    // persisted snapshot exists (e.g. a re-run of an already-finalized turn).
    useAgentStore.setState({
      sessionBlocks: new Map([['s1', [{ kind: 'text', content: 'live stream' }]]]),
    } as never);

    render(
      <AgentMessage
        session={{ ...base, blocks: [{ kind: 'text', content: 'stale persisted' }] }}
        running={false}
        qualityReport={noReport}
      />
    );

    expect(screen.getByText('live stream')).toBeInTheDocument();
    expect(screen.queryByText('stale persisted')).not.toBeInTheDocument();
  });
});

describe('AgentMessage — copy session id', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useAgentStore.setState({ sessionBlocks: new Map() } as never);
  });

  it('copies the session id to clipboard on click and shows 已复制 feedback', async () => {
    // session.id is the unique key for cross-referencing backend logs / the DB;
    // a one-click copy lets the user hand it to whoever is debugging instead of
    // pasting the raw prompt and hoping it's unique enough to grep.
    const writeText = vi.fn().mockResolvedValue(undefined);
    // jsdom ships no clipboard API — inject a stub.
    Object.assign(navigator, { clipboard: { writeText } });
    vi.mocked(invoke).mockResolvedValue('full output');

    render(<AgentMessage session={{ ...base, id: 'sid-copy-test-123' }} running={false} qualityReport={noReport} />);

    const btn = screen.getByRole('button', { name: /复制ID/ });
    await fireEvent.click(btn);

    expect(writeText).toHaveBeenCalledWith('sid-copy-test-123');
    // The button label flips to the confirmation while the timeout runs.
    expect(await screen.findByText('已复制')).toBeInTheDocument();
  });
});
