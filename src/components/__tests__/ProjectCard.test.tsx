import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ProjectCard } from '../ProjectCard';
import { ToastProvider } from '../Toast';
import type { Project } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

const renderWithToast = (ui: React.ReactElement) => render(<ToastProvider>{ui}</ToastProvider>);

const mockProject: Project = {
  id: 'test-id',
  name: 'My Project',
  description: 'A test project',
  path: '/home/user/projects/my-project',
  tags: ['react', 'typescript'],
  cover_image: null,
  open_count: 3,
  last_opened_at: '2025-06-01T10:00:00.000Z',
  starred: false,
  created_at: '2024-01-01T00:00:00.000Z',
  last_opened_tools: [],
  workspace_tools: [],
};

const defaultProps = {
  project: mockProject,
  gitStatus: null,
  isInstalled: vi.fn().mockReturnValue(false),
  onToolOpen: vi.fn(),
  onEdit: vi.fn(),
  onRemove: vi.fn(),
  onToggleStar: vi.fn(),
};

describe('ProjectCard', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders project name and path', () => {
    renderWithToast(<ProjectCard {...defaultProps} />);

    expect(screen.getByText('My Project')).toBeInTheDocument();
    expect(screen.getByText('/home/user/projects/my-project')).toBeInTheDocument();
  });

  it('renders project description', () => {
    renderWithToast(<ProjectCard {...defaultProps} />);

    expect(screen.getByText('A test project')).toBeInTheDocument();
  });

  it('renders tags', () => {
    renderWithToast(<ProjectCard {...defaultProps} />);

    expect(screen.getByText('react')).toBeInTheDocument();
    expect(screen.getByText('typescript')).toBeInTheDocument();
  });

  it('renders open count when > 0', () => {
    renderWithToast(<ProjectCard {...defaultProps} />);

    expect(screen.getByText('打开 3 次')).toBeInTheDocument();
  });

  it('does not render open count when 0', () => {
    const project = { ...mockProject, open_count: 0 };
    renderWithToast(<ProjectCard {...defaultProps} project={project} />);

    expect(screen.queryByText(/打开/)).not.toBeInTheDocument();
  });

  it('renders last opened time', () => {
    renderWithToast(<ProjectCard {...defaultProps} />);

    // last_opened_at is formatted with toLocaleString('zh-CN')
    expect(screen.getByText(/2025/)).toBeInTheDocument();
  });

  it('renders "尚未打开" when last_opened_at is null', () => {
    const project = { ...mockProject, last_opened_at: null };
    renderWithToast(<ProjectCard {...defaultProps} project={project} />);

    expect(screen.getByText('尚未打开')).toBeInTheDocument();
  });

  it('calls onToggleStar when star button is clicked', async () => {
    const user = userEvent.setup();
    renderWithToast(<ProjectCard {...defaultProps} />);

    const starBtn = screen.getByTitle('收藏');
    await user.click(starBtn);

    expect(defaultProps.onToggleStar).toHaveBeenCalledWith('test-id');
  });

  it('shows "取消收藏" title when starred', () => {
    const project = { ...mockProject, starred: true };
    renderWithToast(<ProjectCard {...defaultProps} project={project} />);

    expect(screen.getByTitle('取消收藏')).toBeInTheDocument();
  });

  it('calls onEdit when edit button is clicked', async () => {
    const user = userEvent.setup();
    renderWithToast(<ProjectCard {...defaultProps} />);

    const editBtn = screen.getByTitle('编辑');
    await user.click(editBtn);

    expect(defaultProps.onEdit).toHaveBeenCalledWith(mockProject);
  });

  it('calls onRemove when delete button is clicked', async () => {
    const user = userEvent.setup();
    renderWithToast(<ProjectCard {...defaultProps} />);

    const deleteBtn = screen.getByTitle('删除');
    await user.click(deleteBtn);

    expect(defaultProps.onRemove).toHaveBeenCalledWith('test-id');
  });

  it('renders cover placeholder with first 2 characters of name', () => {
    renderWithToast(<ProjectCard {...defaultProps} />);

    expect(screen.getByText('MY')).toBeInTheDocument();
  });

  it('renders cover image when available', () => {
    const project = { ...mockProject, cover_image: 'test-cover.png' };
    renderWithToast(<ProjectCard {...defaultProps} project={project} />);

    const img = screen.getByRole('img');
    expect(img).toHaveAttribute('src', 'test-cover.png');
  });

  it('shows agent tools from discovery data and non-agent tools', () => {
    const isInstalled = vi.fn().mockImplementation((name: string) => name === 'finder');
    const agents = [
      { agentType: 'claude_code' as const, displayName: 'Claude Code', commandName: 'claude', installed: true, path: '/usr/bin/claude', supportsResume: true },
      { agentType: 'codex' as const, displayName: 'Codex', commandName: 'codex', installed: false, path: null, supportsResume: true },
    ];
    renderWithToast(<ProjectCard {...defaultProps} isInstalled={isInstalled} agents={agents} />);

    // Installed agent shows as enabled
    expect(screen.getByTitle('用 Claude Code 打开')).toBeEnabled();
    // Uninstalled agent is NOT shown (only installed agents render)
    expect(screen.queryByTitle(/Codex/)).not.toBeInTheDocument();
    // Non-agent tools: finder is always installed
    expect(screen.getByTitle('用 Files 打开')).toBeEnabled();
  });

  it('shows only installed agents when agents prop is provided', () => {
    const isInstalled = vi.fn().mockReturnValue(true);
    const agents = [
      { agentType: 'claude_code' as const, displayName: 'Claude Code', commandName: 'claude', installed: true, path: '/usr/bin/claude', supportsResume: true },
    ];
    renderWithToast(<ProjectCard {...defaultProps} isInstalled={isInstalled} agents={agents} />);

    // Should show Claude Code (installed agent) and non-agent tools (VSCode, Files)
    expect(screen.getByTitle('用 Claude Code 打开')).toBeInTheDocument();
    expect(screen.getByTitle('用 Files 打开')).toBeInTheDocument();
    // Codex is not in agents list, should not appear
    expect(screen.queryByTitle(/Codex/)).not.toBeInTheDocument();
  });

  it('calls onToolOpen with tool name when tool button is clicked', async () => {
    const user = userEvent.setup();
    renderWithToast(<ProjectCard {...defaultProps} />);

    const finderBtn = screen.getByTitle('用 Files 打开');
    await user.click(finderBtn);

    expect(defaultProps.onToolOpen).toHaveBeenCalledWith('test-id', 'finder');
  });
});
