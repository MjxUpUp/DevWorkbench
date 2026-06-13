import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { TitleBar } from '../layout/TitleBar';
import { useNavigationStore } from '../../stores/navigationStore';

/**
 * Brand-mark-as-sidebar-toggle coverage.
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

describe('TitleBar — brand mark toggles the left column', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // Reset store so each test starts with the sidebar open.
    useNavigationStore.setState({ sidebarOpen: true });
  });

  it('renders the brand mark with the collapse tooltip when the sidebar is open', () => {
    render(<TitleBar />);
    const brand = screen.getByRole('button', { name: '切换边栏' });
    expect(brand).toBeInTheDocument();
    expect(brand).toHaveAttribute('title', '收起边栏');
    expect(brand).toHaveAttribute('aria-expanded', 'true');
  });

  it('toggles to the expand tooltip after clicking the brand mark', async () => {
    const user = userEvent.setup();
    render(<TitleBar />);
    const brand = screen.getByRole('button', { name: '切换边栏' });

    await user.click(brand);

    // sidebarOpen flipped to false → tooltip now invites expanding.
    expect(screen.getByRole('button', { name: '切换边栏' })).toHaveAttribute('title', '展开边栏');
    expect(useNavigationStore.getState().sidebarOpen).toBe(false);
  });

  it('restores the collapse tooltip when clicked again', async () => {
    const user = userEvent.setup();
    render(<TitleBar />);
    const brand = screen.getByRole('button', { name: '切换边栏' });

    await user.click(brand); // collapse
    await user.click(brand); // expand

    expect(screen.getByRole('button', { name: '切换边栏' })).toHaveAttribute('title', '收起边栏');
    expect(useNavigationStore.getState().sidebarOpen).toBe(true);
  });
});
