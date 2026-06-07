import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ToolButton } from '../ToolButton';
import { ToastProvider } from '../Toast';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
const mockedInvoke = vi.mocked(invoke);

const renderWithToast = (ui: React.ReactElement) => render(<ToastProvider>{ui}</ToastProvider>);

describe('ToolButton', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders tool label', () => {
    renderWithToast(<ToolButton tool="claude" projectPath="/test" installed={true} />);

    expect(screen.getByText('Claude')).toBeInTheDocument();
  });

  it('renders correct labels for each tool', () => {
    renderWithToast(<ToolButton tool="cursor" projectPath="/test" installed={true} />);
    expect(screen.getByText('Cursor')).toBeInTheDocument();

    renderWithToast(<ToolButton tool="code" projectPath="/test" installed={true} />);
    expect(screen.getByText('VSCode')).toBeInTheDocument();

    renderWithToast(<ToolButton tool="terminal" projectPath="/test" installed={true} />);
    expect(screen.getByText('Term')).toBeInTheDocument();

    renderWithToast(<ToolButton tool="finder" projectPath="/test" installed={true} />);
    expect(screen.getByText('Files')).toBeInTheDocument();
  });

  it('is clickable when installed', () => {
    renderWithToast(<ToolButton tool="claude" projectPath="/test" installed={true} />);

    const btn = screen.getByRole('button');
    expect(btn).toBeEnabled();
  });

  it('is disabled when not installed', () => {
    renderWithToast(<ToolButton tool="claude" projectPath="/test" installed={false} />);

    const btn = screen.getByRole('button');
    expect(btn).toBeDisabled();
  });

  it('shows installed title when installed', () => {
    renderWithToast(<ToolButton tool="claude" projectPath="/test" installed={true} />);

    expect(screen.getByTitle('用 Claude 打开')).toBeInTheDocument();
  });

  it('shows uninstalled title when not installed', () => {
    renderWithToast(<ToolButton tool="claude" projectPath="/test" installed={false} />);

    expect(screen.getByTitle('Claude 未安装')).toBeInTheDocument();
  });

  it('invokes open_terminal with command for claude tool', async () => {
    mockedInvoke.mockResolvedValueOnce(undefined);
    const onClick = vi.fn();
    const user = userEvent.setup();

    renderWithToast(<ToolButton tool="claude" projectPath="/my/project" installed={true} onClick={onClick} />);

    await user.click(screen.getByRole('button'));

    expect(mockedInvoke).toHaveBeenCalledWith('open_terminal', {
      workingDir: '/my/project',
      command: 'claude',
    });
    expect(onClick).toHaveBeenCalledWith('claude');
  });

  it('invokes open_terminal without command for terminal tool', async () => {
    mockedInvoke.mockResolvedValueOnce(undefined);
    const user = userEvent.setup();

    renderWithToast(<ToolButton tool="terminal" projectPath="/my/project" installed={true} />);

    await user.click(screen.getByRole('button'));

    expect(mockedInvoke).toHaveBeenCalledWith('open_terminal', {
      workingDir: '/my/project',
    });
  });

  it('invokes open_in_editor for cursor tool', async () => {
    mockedInvoke.mockResolvedValueOnce(undefined);
    const user = userEvent.setup();

    renderWithToast(<ToolButton tool="cursor" projectPath="/my/project" installed={true} />);

    await user.click(screen.getByRole('button'));

    expect(mockedInvoke).toHaveBeenCalledWith('open_in_editor', {
      editor: 'cursor',
      projectPath: '/my/project',
    });
  });

  it('invokes open_in_editor for code tool', async () => {
    mockedInvoke.mockResolvedValueOnce(undefined);
    const user = userEvent.setup();

    renderWithToast(<ToolButton tool="code" projectPath="/my/project" installed={true} />);

    await user.click(screen.getByRole('button'));

    expect(mockedInvoke).toHaveBeenCalledWith('open_in_editor', {
      editor: 'code',
      projectPath: '/my/project',
    });
  });

  it('invokes open_in_finder for finder tool', async () => {
    mockedInvoke.mockResolvedValueOnce(undefined);
    const user = userEvent.setup();

    renderWithToast(<ToolButton tool="finder" projectPath="/my/project" installed={true} />);

    await user.click(screen.getByRole('button'));

    expect(mockedInvoke).toHaveBeenCalledWith('open_in_finder', {
      path: '/my/project',
    });
  });

  it('does not invoke when clicked but not installed', async () => {
    const user = userEvent.setup();

    renderWithToast(<ToolButton tool="claude" projectPath="/my/project" installed={false} />);

    await user.click(screen.getByRole('button'));

    expect(mockedInvoke).not.toHaveBeenCalled();
  });
});
