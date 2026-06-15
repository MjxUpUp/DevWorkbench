import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
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
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders the FULL output from read_session_output_cmd, not the truncated summary', async () => {
    // Backend returns a long, untruncated reply. outputSummary is the 2000-char
    // tail of the SAME log — rendering it would duplicate + cut off. The block
    // must show the full text instead.
    const fullReply = 'A'.repeat(5000);
    vi.mocked(invoke).mockResolvedValue(fullReply);

    render(<AgentMessage session={{ ...base, outputSummary: '...tail' }} running={false} qualityReport={noReport} />);

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

    render(<AgentMessage session={{ ...base, outputSummary: 'only summary left' }} running={false} qualityReport={noReport} />);

    await waitFor(() => {
      expect(screen.getByText('only summary left')).toBeInTheDocument();
    });
  });

  it('does not call read_session_output_cmd while running (live terminal stream instead)', () => {
    const runningSession: Session = { ...base, status: 'running', outputSummary: null, finishedAt: null, exitCode: null };
    render(<AgentMessage session={runningSession} running={true} qualityReport={noReport} />);

    // Running sessions stream via TerminalView; the full-output fetch must NOT fire.
    expect(vi.mocked(invoke)).not.toHaveBeenCalledWith('read_session_output_cmd', expect.anything());
    expect(screen.getByTestId('terminal-stub')).toBeInTheDocument();
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

    render(<AgentMessage session={base} running={false} qualityReport={noReport} />);

    // Output not ready yet (promise pending) → terminal placeholder must be mounted.
    expect(screen.getByTestId('terminal-stub')).toBeInTheDocument();
  });
});
