import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { AgentSection } from '../AgentSection';
import { useAgentStore } from '../../../stores/agentStore';
import { useSettingsStore } from '../../../stores/settingsStore';

/**
 * Regression guard for the settings-page crash: AgentSection used to do
 * `.then(setTools)` on `detect_tools`, so when the backend (or, in tests, a
 * mock) returned a non-array — null, an object, a string — the later
 * `tools.filter(...)` threw `tools.filter is not a function` and the
 * ErrorBoundary ate the entire settings view. The fix wraps the setter in an
 * `Array.isArray` guard. These tests pin that guard by feeding the exact
 * non-array shapes that used to crash.
 */
const mockInvoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));

describe('AgentSection crash defense', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    useAgentStore.setState({ agents: [] });
    useSettingsStore.setState({
      settings: {
        scan_directories: [],
        tool_paths: {},
        theme: 'auto',
        palette: 'pi' as const,
        cli_flags: {},
        onboarding_completed: true,
      },
      error: null,
    });
  });

  it('does not crash when detect_tools returns null', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'detect_tools') return Promise.resolve(null);
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });

    // Must not throw during render or effect flush.
    expect(() => render(<AgentSection />)).not.toThrow();

    expect(await screen.findByText('OpaqueAgent 二进制路径（高级）')).toBeInTheDocument();
    expect(mockInvoke).toHaveBeenCalledWith('detect_tools');
  });

  it('does not crash when detect_tools rejects', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'detect_tools') return Promise.reject(new Error('boom'));
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });

    expect(() => render(<AgentSection />)).not.toThrow();
    expect(await screen.findByText('OpaqueAgent 二进制路径（高级）')).toBeInTheDocument();
  });

  it('does not crash when detect_tools returns a non-array object', async () => {
    // Backend returning `{ error: '...' }` instead of an array — the original
    // real-world trigger (TS believed ToolStatus[] but runtime got an object).
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'detect_tools') return Promise.resolve({ error: 'nope' });
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });

    expect(() => render(<AgentSection />)).not.toThrow();
    expect(await screen.findByText('OpaqueAgent 二进制路径（高级）')).toBeInTheDocument();
  });
});
