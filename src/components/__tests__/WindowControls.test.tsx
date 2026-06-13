import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { WindowControls } from '../layout/WindowControls';

const { mockMinimize, mockToggleMaximize, mockClose } = vi.hoisted(() => ({
  mockMinimize: vi.fn().mockResolvedValue(undefined),
  mockToggleMaximize: vi.fn().mockResolvedValue(undefined),
  mockClose: vi.fn().mockResolvedValue(undefined),
}));

vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({
    minimize: mockMinimize,
    toggleMaximize: mockToggleMaximize,
    close: mockClose,
  }),
}));

vi.mock('../../utils/env', () => ({
  isTauri: () => true,
}));

describe('WindowControls', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders minimize / maximize / close buttons', () => {
    render(<WindowControls maximized={false} />);
    expect(screen.getByTitle('最小化')).toBeInTheDocument();
    expect(screen.getByTitle('最大化')).toBeInTheDocument();
    expect(screen.getByTitle('关闭')).toBeInTheDocument();
  });

  it('shows the restore control when maximized (and hides maximize)', () => {
    render(<WindowControls maximized={true} />);
    expect(screen.getByTitle('还原')).toBeInTheDocument();
    expect(screen.queryByTitle('最大化')).not.toBeInTheDocument();
  });

  it('calls minimize() when the minimize button is clicked', async () => {
    const user = userEvent.setup();
    render(<WindowControls maximized={false} />);
    await user.click(screen.getByTitle('最小化'));
    expect(mockMinimize).toHaveBeenCalledOnce();
  });

  it('calls toggleMaximize() when the maximize button is clicked', async () => {
    const user = userEvent.setup();
    render(<WindowControls maximized={false} />);
    await user.click(screen.getByTitle('最大化'));
    expect(mockToggleMaximize).toHaveBeenCalledOnce();
  });

  it('calls close() when the close button is clicked', async () => {
    const user = userEvent.setup();
    render(<WindowControls maximized={false} />);
    await user.click(screen.getByTitle('关闭'));
    expect(mockClose).toHaveBeenCalledOnce();
  });
});
