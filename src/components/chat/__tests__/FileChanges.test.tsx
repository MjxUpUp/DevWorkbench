import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('../../Icons', () => ({ IconEdit: () => <svg data-testid="icon-edit" /> }));

import { invoke } from '@tauri-apps/api/core';
import { FileChanges } from '../FileChanges';
import type { Session } from '../../../types';

const base = {
  id: 's1',
  projectPath: '/p',
  agentType: 'claude_code' as const,
  status: 'completed' as const,
  prompt: '',
  model: null,
  startedAt: '2026-01-01T00:00:00Z',
  finishedAt: null,
  exitCode: 0,
  outputSummary: null,
  contextSnapshot: null,
  linkedRequirementId: null,
  parentSessionId: null,
  conversationId: null,
} satisfies Session;

const withFiles = (files: string[]): Session => ({
  ...base,
  contextSnapshot: { filesChanged: files, keyOutput: '' },
});

describe('FileChanges — shadow-git rollback button (v1.2 T6)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.unstubAllGlobals();
  });

  it('renders the changed-files list', async () => {
    vi.mocked(invoke).mockResolvedValue(null); // get_checkpoint → no checkpoint
    render(<FileChanges session={withFiles(['a.rs', 'b.ts'])} />);
    expect(await screen.findByText('a.rs')).toBeInTheDocument();
    expect(screen.getByText('b.ts')).toBeInTheDocument();
  });

  it('shows the rollback button when a checkpoint exists', async () => {
    vi.mocked(invoke).mockResolvedValue({ sessionId: 's1', headSha: 'abc' });
    render(<FileChanges session={withFiles(['a.rs'])} />);
    expect(await screen.findByText(/回滚改动/)).toBeInTheDocument();
  });

  it('hides the rollback button when no checkpoint exists', async () => {
    vi.mocked(invoke).mockResolvedValue(null);
    render(<FileChanges session={withFiles(['a.rs'])} />);
    await waitFor(() => {
      expect(screen.queryByText(/回滚改动/)).not.toBeInTheDocument();
    });
  });

  it('invokes rollback_to_checkpoint after the user confirms', async () => {
    vi.mocked(invoke)
      .mockResolvedValueOnce({ sessionId: 's1' }) // get_checkpoint (probe)
      .mockResolvedValueOnce({ restoredFiles: ['a.rs'], removedUntracked: [], skipped: [] }); // rollback
    vi.stubGlobal('confirm', () => true);
    render(<FileChanges session={withFiles(['a.rs'])} />);
    fireEvent.click(await screen.findByText(/回滚改动/));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith(
        'rollback_to_checkpoint',
        expect.objectContaining({ sessionId: 's1', force: false }),
      );
    });
    // Success renders the result summary.
    expect(await screen.findByText(/已回滚/)).toBeInTheDocument();
  });

  it('does not roll back when the user cancels the confirm', async () => {
    vi.mocked(invoke).mockResolvedValue({ sessionId: 's1' }); // probe only
    vi.stubGlobal('confirm', () => false);
    render(<FileChanges session={withFiles(['a.rs'])} />);
    fireEvent.click(await screen.findByText(/回滚改动/));
    // Only the get_checkpoint probe ran; no rollback_to_checkpoint call.
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledTimes(1);
    });
    expect(invoke).not.toHaveBeenCalledWith(
      'rollback_to_checkpoint',
      expect.anything(),
    );
  });

  it('renders nothing when there are no changed files and no result', () => {
    vi.mocked(invoke).mockResolvedValue(null);
    const { container } = render(<FileChanges session={base} />);
    expect(container).toBeEmptyDOMElement();
  });
});
