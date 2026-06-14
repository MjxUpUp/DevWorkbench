import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { SettingsView } from '../settings/SettingsView';
import { useNavigationStore } from '../../stores/navigationStore';
import { useSettingsStore } from '../../stores/settingsStore';
import { invoke } from '@tauri-apps/api/core';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

describe('SettingsView — full-screen overlay', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // loadSettings calls invoke('load_settings'); resolve with the store default.
    // Other commands (e.g. AgentSection's discover_agents_cmd) get [] so list
    // rendering in the default section doesn't throw on a non-array.
    vi.mocked(invoke).mockImplementation((cmd) => {
      if (String(cmd) === 'load_settings') {
        return Promise.resolve(useSettingsStore.getState().settings);
      }
      return Promise.resolve([]);
    });
    useNavigationStore.setState({ activeView: 'settings' });
  });

  it('renders the overlay with a heading and an accessible close control', () => {
    render(<SettingsView />);
    expect(screen.getByRole('heading', { name: '设置' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '关闭设置' })).toBeInTheDocument();
  });

  it('returns to the task view on Escape', () => {
    render(<SettingsView />);
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(useNavigationStore.getState().activeView).toBe('task');
  });

  it('returns to the task view when the close button is clicked', () => {
    render(<SettingsView />);
    fireEvent.click(screen.getByRole('button', { name: '关闭设置' }));
    expect(useNavigationStore.getState().activeView).toBe('task');
  });
});
