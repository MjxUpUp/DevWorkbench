import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { ActivityBar } from '../ActivityBar';
import { useNavigationStore } from '../../../stores/navigationStore';

/**
 * ActivityBar — 起底重构后视图导航的唯一入口（从 Sidebar 顶部 nav 抽出）。
 * 6 个图标（新建会话 / 添加工作区 / Task / Trace / 搜索 / 设置）。这些测试钉住：图标都在 /
 * 激活态跟随 activeView / 各点击的副作用。
 */
describe('ActivityBar', () => {
  beforeEach(() => {
    useNavigationStore.setState({
      activeView: 'task',
      selectedConversationId: 'conv-1',
      commandPaletteOpen: false,
    });
  });

  it('renders all six nav icons', () => {
    render(<ActivityBar />);
    expect(screen.getByTestId('ab-new')).toBeInTheDocument();
    expect(screen.getByTestId('ab-add-workspace')).toBeInTheDocument();
    expect(screen.getByTestId('ab-task')).toBeInTheDocument();
    expect(screen.getByTestId('ab-trace')).toBeInTheDocument();
    expect(screen.getByTestId('ab-search')).toBeInTheDocument();
    expect(screen.getByTestId('ab-settings')).toBeInTheDocument();
  });

  it('marks the active view icon with .active', () => {
    render(<ActivityBar />);
    expect(screen.getByTestId('ab-task')).toHaveClass('active');
    expect(screen.getByTestId('ab-trace')).not.toHaveClass('active');
  });

  it('switches active marker when activeView changes', () => {
    useNavigationStore.setState({ activeView: 'trace' });
    render(<ActivityBar />);
    expect(screen.getByTestId('ab-trace')).toHaveClass('active');
    expect(screen.getByTestId('ab-task')).not.toHaveClass('active');
  });

  it('switches to trace on click', () => {
    render(<ActivityBar />);
    fireEvent.click(screen.getByTestId('ab-trace'));
    expect(useNavigationStore.getState().activeView).toBe('trace');
  });

  it('opens settings on click', () => {
    render(<ActivityBar />);
    fireEvent.click(screen.getByTestId('ab-settings'));
    expect(useNavigationStore.getState().activeView).toBe('settings');
  });

  it('opens the command palette on search click', () => {
    render(<ActivityBar />);
    fireEvent.click(screen.getByTestId('ab-search'));
    expect(useNavigationStore.getState().commandPaletteOpen).toBe(true);
  });

  it('new chat clears the selected conversation and stays in task view', () => {
    render(<ActivityBar />);
    fireEvent.click(screen.getByTestId('ab-new'));
    expect(useNavigationStore.getState().selectedConversationId).toBeNull();
    expect(useNavigationStore.getState().activeView).toBe('task');
  });

  it('add-workspace opens the AddProject modal', () => {
    render(<ActivityBar />);
    fireEvent.click(screen.getByTestId('ab-add-workspace'));
    expect(useNavigationStore.getState().addProjectOpen).toBe(true);
  });
});
