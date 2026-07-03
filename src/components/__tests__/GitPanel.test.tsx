import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { GitPanel } from '../git/GitPanel';
import type { GitStatus, ChangedFile, GitFileDiff } from '../../types';

/**
 * GitPanel reads git status + the changed-file list via Tauri invoke and
 * reports the result. These tests stub the Tauri invoke bridge (command-aware
 * so get_git_status / list_changed_files / get_file_diff each return their own
 * shape) and the Toast context (no provider in the test tree) so the component
 * renders deterministically.
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

/** Command-aware mock: dispatches by command name so the two Promise.all'd
 *  calls each get the right shape (status object vs file array). */
function mockCommands(opts: { status?: GitStatus; files?: ChangedFile[]; diff?: GitFileDiff }) {
  mockInvoke.mockImplementation(async (cmd: string) => {
    if (cmd === 'get_git_status') return opts.status ?? clean;
    if (cmd === 'list_changed_files') return opts.files ?? [];
    if (cmd === 'get_file_diff') return opts.diff ?? { path: '', hunks: [], isBinary: false };
    return undefined;
  });
}

describe('GitPanel', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it('prompts to select a project when no path is given', () => {
    render(<GitPanel projectPath={null} />);
    expect(screen.getByText('选择工作区后查看 Git 状态')).toBeInTheDocument();
  });

  it('shows the insertions/deletions counts for a dirty repo', async () => {
    mockCommands({ status: dirty });
    render(<GitPanel projectPath="/repo" />);

    await waitFor(() => {
      expect(screen.getByText('+42')).toBeInTheDocument();
      expect(screen.getByText('-7')).toBeInTheDocument();
    });
    expect(screen.getByText('feature/x')).toBeInTheDocument();
  });

  it('disables the commit button when the working tree is clean', async () => {
    mockCommands({ status: clean });
    render(<GitPanel projectPath="/repo" />);

    await waitFor(() => {
      const commit = screen.getByRole('button', { name: /提交/ });
      expect(commit).toBeDisabled();
    });
  });

  it('opens a terminal (workingDir) when committing a dirty repo', async () => {
    const user = userEvent.setup();
    mockCommands({ status: dirty });
    render(<GitPanel projectPath="/repo" />);

    const commit = await screen.findByRole('button', { name: /提交/ });
    await waitFor(() => expect(commit).not.toBeDisabled());
    await user.click(commit);

    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('open_terminal', { workingDir: '/repo' });
    });
  });

  it('renders the per-file change list with status badges + counts (A2)', async () => {
    mockCommands({
      status: dirty,
      files: [
        { path: 'src/a.ts', status: 'M', added: 3, removed: 1 },
        { path: 'new.txt', status: 'U', added: 10, removed: 0 },
      ],
    });
    render(<GitPanel projectPath="/repo" />);

    expect(await screen.findByText('改动文件 (2)')).toBeInTheDocument();
    expect(screen.getByText('src/a.ts')).toBeInTheDocument();
    expect(screen.getByText('new.txt')).toBeInTheDocument();
    // Modified badge + untracked badge.
    expect(screen.getByText('M')).toBeInTheDocument();
    expect(screen.getByText('U')).toBeInTheDocument();
    // Per-file counts (the +3 on the modified file).
    expect(screen.getAllByText('+3').length).toBeGreaterThan(0);
  });

  it('expanding a file fetches get_file_diff and renders colored hunks (A2)', async () => {
    const user = userEvent.setup();
    const diff: GitFileDiff = {
      path: 'src/a.ts',
      isBinary: false,
      hunks: [
        { kind: 'meta', text: '@@ -1,1 +1,2 @@ hunk', oldNo: null, newNo: null },
        { kind: 'context', text: 'keep', oldNo: 1, newNo: 1 },
        { kind: 'remove', text: 'old line', oldNo: 2, newNo: null },
        { kind: 'add', text: 'NEW LINE', oldNo: null, newNo: 2 },
      ],
    };
    mockCommands({ status: dirty, files: [{ path: 'src/a.ts', status: 'M', added: 1, removed: 1 }], diff });

    render(<GitPanel projectPath="/repo" />);
    const row = await screen.findByRole('button', { name: /src\/a\.ts/ });
    // Hunk content is NOT rendered before expand.
    expect(screen.queryByText('NEW LINE')).not.toBeInTheDocument();

    await user.click(row);

    // get_file_diff is called with the file path.
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('get_file_diff', { projectPath: '/repo', filePath: 'src/a.ts' });
    });
    // The added + removed lines render after expand.
    expect(await screen.findByText('NEW LINE')).toBeInTheDocument();
    expect(screen.getByText('old line')).toBeInTheDocument();
  });

  it('shows a binary badge for non-text diffs (A2)', async () => {
    const user = userEvent.setup();
    mockCommands({
      status: dirty,
      files: [{ path: 'logo.png', status: 'A', added: 0, removed: 0 }],
      diff: { path: 'logo.png', isBinary: true, hunks: [] },
    });
    render(<GitPanel projectPath="/repo" />);
    const row = await screen.findByRole('button', { name: /logo\.png/ });
    await user.click(row);
    expect(await screen.findByText('二进制文件，无法显示文本差异')).toBeInTheDocument();
  });
});
