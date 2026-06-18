import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { AgentMessage } from '../AgentMessage';
import { useAgentStore } from '../../../stores/agentStore';
import type { Session, QualityReport } from '../../../types';

// Mock invoke so we can assert the completed-session path loads the FULL output
// (read_session_output_cmd) rather than the tail-truncated outputSummary.
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
// TerminalView pulls in xterm + the event API; stub it so the test stays focused.
vi.mock('../../TerminalView', () => ({
  TerminalView: ({ sessionId }: { sessionId: string | null }) => (
    <div data-testid="terminal-stub">{sessionId ?? 'no-session'}</div>
  ),
}));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

const base: Session = {
  id: 's1',
  projectPath: '/p',
  agentType: 'claude_code',
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

describe('AgentMessage — completed-session output', () => {
  // The full-output / terminal-placeholder / outputSummary paths below belong
  // ONLY to raw agents (pi/codex): they emit pty bytes, never agent:event
  // blocks. Structured agents (claude_code/react_kernel/gemini_cli/qwen_code)
  // render BlocksView in EVERY state (running-empty → waiting hint, running
  // accumulating, completed → persisted blocks) and NEVER touch these paths —
  // that's the terminal-flicker fix. So these terminal-path cases use
  // agentType: 'pi' (raw); the structured path has its own tests below.
  beforeEach(() => {
    vi.clearAllMocks();
    // Reset only the in-memory block map (new in this change). ptyOutput is
    // left untouched to preserve the existing tests' assumptions.
    useAgentStore.setState({ sessionBlocks: new Map() } as never);
  });

  it('renders the FULL output from read_session_output_cmd, not the truncated summary', async () => {
    // Backend returns a long, untruncated reply. outputSummary is the 2000-char
    // tail of the SAME log — rendering it would duplicate + cut off. The block
    // must show the full text instead.
    const fullReply = 'A'.repeat(5000);
    vi.mocked(invoke).mockResolvedValue(fullReply);

    render(<AgentMessage session={{ ...base, agentType: 'pi', outputSummary: '...tail' }} running={false} qualityReport={noReport} />);

    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('read_session_output_cmd', { sessionId: 's1' });
    });
    // The rendered output must be the FULL reply (5000 A's), never the truncated tail.
    await waitFor(() => {
      expect(screen.getByText('A'.repeat(5000))).toBeInTheDocument();
      expect(screen.queryByText('...tail')).not.toBeInTheDocument();
    });
  });

  it('falls back to outputSummary when the full log is unavailable', async () => {
    // Log file gone (old session) → backend returns null → render the summary
    // so the user still sees something rather than an empty reply.
    vi.mocked(invoke).mockResolvedValue(null);

    render(<AgentMessage session={{ ...base, agentType: 'pi', outputSummary: 'only summary left' }} running={false} qualityReport={noReport} />);

    await waitFor(() => {
      expect(screen.getByText('only summary left')).toBeInTheDocument();
    });
  });

  it('structured agent running shows chat-blocks waiting, NOT terminal (no full-output fetch)', () => {
    // claude_code (and react_kernel) are structured: while running with no block
    // yet (e.g. the model gateway holding its response) they must render
    // BlocksView's "等待模型响应" waiting state — NEVER the terminal box. This
    // is the regression the fix targets: previously claude fell through to
    // TerminalView here because `running` short-circuited `showTerminal`.
    const runningSession: Session = { ...base, status: 'running', outputSummary: null, finishedAt: null, exitCode: null };
    render(<AgentMessage session={runningSession} running={true} qualityReport={noReport} />);

    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('read_session_output_cmd', expect.anything());
    expect(screen.queryByTestId('terminal-stub')).not.toBeInTheDocument();
    expect(screen.getByText('等待模型响应')).toBeInTheDocument();
  });

  it('raw agent (pi) running keeps the terminal stream, not chat-blocks', () => {
    // Raw agents emit only pty:output bytes → no agent:event blocks → they keep
    // the terminal path while running. Guards the structured/raw split so the
    // fix doesn't accidentally force pi/codex into BlocksView waiting.
    const runningPi: Session = { ...base, agentType: 'pi', status: 'running', outputSummary: null, finishedAt: null, exitCode: null };
    render(<AgentMessage session={runningPi} running={true} qualityReport={noReport} />);

    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('read_session_output_cmd', expect.anything());
    expect(screen.getByTestId('terminal-stub')).toBeInTheDocument();
    expect(screen.queryByText('等待模型响应')).not.toBeInTheDocument();
  });

  it('keeps the terminal as a placeholder while the full output loads (no flash)', async () => {
    // Regression: the moment a session completes, `running` flips false but the
    // full-output invoke hasn't resolved yet. Previously TerminalView was torn
    // down at that instant, leaving an xterm canvas residue that flashed before
    // markdown arrived. With pty cache present the terminal must STAY mounted as
    // a placeholder until markdown is ready (same-commit swap, no flash).
    useAgentStore.setState({
      ptyOutput: new Map([['s1', [new Uint8Array([0x78, 0x74, 0x65, 0x72, 0x6d])]]]),
    } as never);
    // Never-resolving promise simulates the load window.
    vi.mocked(invoke).mockReturnValue(new Promise(() => {}));

    render(<AgentMessage session={{ ...base, agentType: 'pi' }} running={false} qualityReport={noReport} />);

    // Output not ready yet (promise pending) → terminal placeholder must be mounted.
    expect(screen.getByTestId('terminal-stub')).toBeInTheDocument();
  });

  it('renders BlocksView when structured blocks are in store (chat-blocks path)', () => {
    // claude (and later ReactAgent) stream via `agent:event`; the accumulated
    // blocks drive BlocksView, NOT the terminal/markdown path. And because
    // blocks are authoritative here, the full-output fetch must be skipped.
    useAgentStore.setState({
      sessionBlocks: new Map([
        ['s1', [
          { kind: 'text', content: 'block reply' },
          { kind: 'tool_use', name: 'Read', input: { file_path: '/x' } },
        ]],
      ]),
    } as never);
    vi.mocked(invoke).mockResolvedValue('should-not-load');

    render(<AgentMessage session={base} running={false} qualityReport={noReport} />);

    expect(screen.getByText('block reply')).toBeInTheDocument();
    expect(screen.getByText('Read')).toBeInTheDocument();
    expect(screen.queryByTestId('terminal-stub')).not.toBeInTheDocument();
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('read_session_output_cmd', expect.anything());
  });

  it('replays persisted session.blocks when the live Map is empty (history reload)', () => {
    // After a reload or project-switch, the live in-memory Map is empty but the
    // completed session carries its persisted blocks from the DB. Those must
    // drive BlocksView — NOT the terminal/full-output log path.
    useAgentStore.setState({ sessionBlocks: new Map() } as never);
    vi.mocked(invoke).mockResolvedValue('should-not-load');

    render(
      <AgentMessage
        session={{ ...base, blocks: [{ kind: 'text', content: 'persisted reply' }] }}
        running={false}
        qualityReport={noReport}
      />
    );

    expect(screen.getByText('persisted reply')).toBeInTheDocument();
    expect(screen.queryByTestId('terminal-stub')).not.toBeInTheDocument();
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('read_session_output_cmd', expect.anything());
  });

  it('prefers live blocks over persisted session.blocks (running session shadowing)', () => {
    // While a session runs, the live Map is authoritative even if a stale
    // persisted snapshot exists (e.g. a re-run of an already-finalized turn).
    useAgentStore.setState({
      sessionBlocks: new Map([['s1', [{ kind: 'text', content: 'live stream' }]]]),
    } as never);
    vi.mocked(invoke).mockResolvedValue('should-not-load');

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

  it('falls back to the terminal path for a raw agent with no blocks (pi/codex regression)', () => {
    // Raw agents emit no agent:event → no live blocks AND no persisted blocks.
    // They must keep the terminal/full-output path. Guards against the blocks
    // refactor silently breaking pi/codex display.
    useAgentStore.setState({ sessionBlocks: new Map() } as never);
    vi.mocked(invoke).mockResolvedValue('raw agent full output');

    render(
      <AgentMessage session={{ ...base, agentType: 'pi', blocks: null }} running={false} qualityReport={noReport} />
    );

    // No blocks → the terminal stub mounts (the full-output path), and the
    // raw text is fetched + rendered.
    expect(screen.getByTestId('terminal-stub')).toBeInTheDocument();
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
