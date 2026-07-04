import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ConversationBookmarks } from '../ConversationBookmarks';
import { useAgentStore } from '../../../stores/agentStore';
import { useNavigationStore } from '../../../stores/navigationStore';
import type { Project, Conversation } from '../../../types';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(() => Promise.resolve(null)) }));

const project: Project = {
  id: 'p1', name: 'Alpha', description: '', path: 'E:/Alpha',
  tags: [], cover_image: null, open_count: 0, last_opened_at: null,
  starred: false, created_at: '2024-01-01T00:00:00.000Z',
  last_opened_tools: [], workspace_tools: [],
};

const conv = (id: string, title: string, pinned = false): Conversation => ({
  id, projectPath: 'E:/Alpha', title, lastAgent: 'react_kernel',
  status: 'active', startedAt: '2026-01-01T00:00:00Z',
  lastActivityAt: '2026-01-01T00:00:00Z', pinned,
});

describe('ConversationBookmarks — 常驻会话书签栏', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useAgentStore.setState({
      conversations: [conv('c1', '登录修复'), conv('c2', '样式调整', true)],
    } as never);
    useNavigationStore.setState({
      activeProject: project,
      selectedConversationId: null,
      activeView: 'task',
    } as never);
  });

  it('每个旧会话渲染一个书签 + 末尾 + 新建', () => {
    render(<ConversationBookmarks project={project} />);
    expect(screen.getByText('登录修复')).toBeInTheDocument();
    expect(screen.getByText('样式调整')).toBeInTheDocument();
    expect(screen.getByTestId('conversation-bookmark-add')).toBeInTheDocument();
  });

  it('pinned 会话排序在前（钉住的「样式调整」排在「登录修复」之前）', () => {
    render(<ConversationBookmarks project={project} />);
    const tabs = screen.getAllByRole('tab');
    expect(tabs[0]).toHaveTextContent('样式调整');
    expect(tabs[1]).toHaveTextContent('登录修复');
  });

  it('点击书签 → selectConversation(id)', async () => {
    const user = userEvent.setup();
    render(<ConversationBookmarks project={project} />);
    await user.click(screen.getByText('登录修复'));
    expect(useNavigationStore.getState().selectedConversationId).toBe('c1');
  });

  it('点击 + 新建 → selectConversation(null)', async () => {
    const user = userEvent.setup();
    useNavigationStore.setState({ selectedConversationId: 'c1' } as never);
    render(<ConversationBookmarks project={project} />);
    await user.click(screen.getByTestId('conversation-bookmark-add'));
    expect(useNavigationStore.getState().selectedConversationId).toBeNull();
  });

  it('点击 × → invoke delete_conversation 传对应 id', async () => {
    const user = userEvent.setup();
    const { invoke } = await import('@tauri-apps/api/core');
    render(<ConversationBookmarks project={project} />);
    await user.click(screen.getByRole('button', { name: '删除会话 登录修复' }));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('delete_conversation', { id: 'c1' });
  });

  it('删除当前选中会话 → 先 selectConversation(null) 再 delete（避免停在已删会话）', async () => {
    const user = userEvent.setup();
    useNavigationStore.setState({ selectedConversationId: 'c1' } as never);
    render(<ConversationBookmarks project={project} />);
    await user.click(screen.getByRole('button', { name: '删除会话 登录修复' }));
    expect(useNavigationStore.getState().selectedConversationId).toBeNull();
  });

  it('running=true → 显示 LiveDot + requestId', () => {
    render(<ConversationBookmarks project={project} running requestId="sess-1" />);
    expect(screen.getByText('sess-1')).toBeInTheDocument();
    expect(document.querySelector('[class*="livedot"]')).not.toBeNull();
  });
});
