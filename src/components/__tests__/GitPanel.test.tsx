import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { GitPanel } from '../git/GitPanel';
import type { GitStatus } from '../../types';

/**
 * GitPanel reads git status via Tauri invoke and reports the result. These
 * tests stub the Tauri invoke bridge and the Toast context (no provider in the
 * test tree) so the component renders deterministically.
 */
const mockInvoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));
vi.mock('../../utils/env', () => ({ isTauri: () => true }));
vi.mock('../Toast', () => ({
  useToast: () => ({ info: vi.fn(), error: vi.fn(), success: vi.fn(), toast: vi.fn() }),
}));

const dirty: GitStatus = {
  branch: 'feature/x',
  isDirty: true,
  ahead: 2,
  behind: 0,
  lastCommitTime: null,
  insertions: 42,
  deletions: 7,
};

const clean: GitStatus = {
  branch: 'main',
  isDirty: false,
  ahead: 0,
  behind: 0,
  lastCommitTime: null,
  insertions: 0,
  deletions: 0,
};

describe('GitPanel', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('prompts to select a project when no path is given', () => {
    render(<GitPanel projectPath={null} />);
    expect(screen.getByText('选择项目后查看 Git 状态')).toBeInTheDocument();
  });

  it('shows the insertions/deletions counts for a dirty repo', async () => {
    mockInvoke.mockResolvedValue(dirty);
    render(<GitPanel projectPath="/repo" />);

    await waitFor(() => {
      expect(screen.getByText('+42')).toBeInTheDocument();
      expect(screen.getByText('-7')).toBeInTheDocument();
    });
    expect(screen.getByText('feature/x')).toBeInTheDocument();
  });

  it('disables the commit button when the working tree is clean', async () => {
    mockInvoke.mockResolvedValue(clean);
    render(<GitPanel projectPath="/repo" />);

    await waitFor(() => {
      const commit = screen.getByRole('button', { name: /提交/ });
      expect(commit).toBeDisabled();
    });
  });

  it('opens a terminal (workingDir) when committing a dirty repo', async () => {
    const user = userEvent.setup();
    mockInvoke.mockResolvedValue(dirty);
    render(<GitPanel projectPath="/repo" />);

    const commit = await screen.findByRole('button', { name: /提交/ });
    await waitFor(() => expect(commit).not.toBeDisabled());
    await user.click(commit);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('open_terminal', { workingDir: '/repo' });
    });
  });
});
