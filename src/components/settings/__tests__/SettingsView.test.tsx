import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SettingsView } from '../SettingsView';

/**
 * SettingsView owns the section nav + routing. This pins the A7 rename:
 * the section formerly id'd "plugins" (label "能力总览") is now "capability",
 * backed by CapabilitySection. The nav must still surface it, clicking it must
 * route to CapabilitySection (which renders the built-in tools overview), and
 * nothing should reference the dead PluginsSection / DashboardView.
 */
const mockInvoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));

function setupInvoke() {
  mockInvoke.mockImplementation((cmd: string) => {
    if (cmd === 'load_settings') return Promise.resolve({});
    if (cmd === 'list_skills') return Promise.resolve([]);
    if (cmd === 'mcp_catalog') return Promise.resolve([]);
    return Promise.resolve(undefined);
  });
}

describe('SettingsView (A7 plugins→capability rename)', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    setupInvoke();
  });

  it('renders the 能力总览 nav entry', () => {
    render(<SettingsView />);
    expect(screen.getByRole('button', { name: '能力总览' })).toBeInTheDocument();
  });

  it('routes to CapabilitySection on click — built-in tools overview appears', async () => {
    const user = userEvent.setup();
    render(<SettingsView />);
    await user.click(screen.getByRole('button', { name: '能力总览' }));
    // CapabilitySection renders the built-in tools group + the dispatch_subagent
    // entry that proves it is the renamed component, not a leftover.
    expect((await screen.findAllByText('内置工具')).length).toBeGreaterThanOrEqual(1);
    expect(screen.getByText('dispatch_subagent', { exact: false })).toBeInTheDocument();
  });

  it('defaults to the agent-tools section (not capability)', () => {
    render(<SettingsView />);
    // The capability overview is NOT the default landing section.
    expect(screen.queryByText('内置工具')).not.toBeInTheDocument();
  });
});
