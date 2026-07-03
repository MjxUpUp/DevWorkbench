import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { TitleBar } from '../layout/TitleBar';
import { useNavigationStore } from '../../stores/navigationStore';

/**
 * Brand-mark coverage. 工作区重构后 brand 按钮从「切换边栏」改为「返回任务视图」
 * （Sidebar 删除，无 left-column 可折叠）。点击 → setActiveView('task') +
 * selectConversation(null)，对齐网页 logo 回首页的肌肉记忆。
 *
 * The TitleBar also touches Tauri APIs (git status via invoke, window maximize state),
 * which are unrelated to this change. We mock them so the component renders without
 * requiring a Tauri runtime, mirroring the WindowControls.test.tsx mock pattern.
 */
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockRejectedValue(new Error('no git')),
}));

const { mockIsMaximized, mockOnResized } = vi.hoisted(() => ({
  mockIsMaximized: vi.fn().mockResolvedValue(false),
  mockOnResized: vi.fn().mockResolvedValue(() => {}),
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    isMaximized: mockIsMaximized,
    onResized: mockOnResized,
  }),
}));

vi.mock('../../utils/env', () => ({
  isTauri: () => true,
}));

describe('TitleBar — brand mark returns to the task view', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useNavigationStore.setState({
      activeView: 'trace',
      selectedConversationId: 'conv-1',
    });
  });

  it('renders the brand mark with the return-to-task tooltip', () => {
    render(<TitleBar />);
    const brand = screen.getByRole('button', { name: '返回任务视图' });
    expect(brand).toBeInTheDocument();
    expect(brand).toHaveAttribute('title', '返回任务视图');
  });

  it('clicking the brand mark switches to the task view and clears the conversation', async () => {
    const user = userEvent.setup();
    render(<TitleBar />);
    const brand = screen.getByRole('button', { name: '返回任务视图' });

    await user.click(brand);

    expect(useNavigationStore.getState().activeView).toBe('task');
    expect(useNavigationStore.getState().selectedConversationId).toBeNull();
  });
});
