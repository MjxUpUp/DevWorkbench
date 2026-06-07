import { describe, it, expect, vi, beforeEach } from 'vitest';
import { launchTool } from '../launchTool';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
const mockedInvoke = vi.mocked(invoke);

const MOCK_SETTINGS = {
  scan_directories: [],
  tool_paths: {},
  theme: 'obsidian',
  preferred_terminal: '',
  cli_flags: {},
};

describe('launchTool', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('launches claude via open_terminal with command', async () => {
    mockedInvoke.mockResolvedValueOnce(MOCK_SETTINGS); // load_settings
    mockedInvoke.mockResolvedValueOnce(undefined);      // open_terminal
    await launchTool('claude', '/my/project');

    expect(mockedInvoke).toHaveBeenCalledWith('open_terminal', {
      workingDir: '/my/project',
      command: 'claude',
    });
  });

  it('launches claude with CLI flags', async () => {
    mockedInvoke.mockResolvedValueOnce({
      ...MOCK_SETTINGS,
      cli_flags: { claude: '--dangerously-skip-permissions' },
    });
    mockedInvoke.mockResolvedValueOnce(undefined);
    await launchTool('claude', '/my/project');

    expect(mockedInvoke).toHaveBeenCalledWith('open_terminal', {
      workingDir: '/my/project',
      command: 'claude --dangerously-skip-permissions',
    });
  });

  it('launches cursor via open_in_editor', async () => {
    mockedInvoke.mockResolvedValueOnce(MOCK_SETTINGS); // load_settings
    mockedInvoke.mockResolvedValueOnce(undefined);      // open_in_editor
    await launchTool('cursor', '/my/project');

    expect(mockedInvoke).toHaveBeenCalledWith('open_in_editor', {
      editor: 'cursor',
      projectPath: '/my/project',
    });
  });

  it('launches code via open_in_editor', async () => {
    mockedInvoke.mockResolvedValueOnce(MOCK_SETTINGS);
    mockedInvoke.mockResolvedValueOnce(undefined);
    await launchTool('code', '/my/project');

    expect(mockedInvoke).toHaveBeenCalledWith('open_in_editor', {
      editor: 'code',
      projectPath: '/my/project',
    });
  });

  it('launches finder via open_in_finder', async () => {
    mockedInvoke.mockResolvedValueOnce(MOCK_SETTINGS);
    mockedInvoke.mockResolvedValueOnce(undefined);
    await launchTool('finder', '/my/project');

    expect(mockedInvoke).toHaveBeenCalledWith('open_in_finder', {
      path: '/my/project',
    });
  });

  it('throws for unknown tool', async () => {
    mockedInvoke.mockResolvedValueOnce(MOCK_SETTINGS); // load_settings still called
    await expect(launchTool('unknown-tool', '/path')).rejects.toThrow('未知工具: unknown-tool');
    // load_settings is called, but open_terminal/open_in_editor should NOT be
    expect(mockedInvoke).toHaveBeenCalledTimes(1);
    expect(mockedInvoke).toHaveBeenCalledWith('load_settings');
  });

  it('works when load_settings fails', async () => {
    mockedInvoke.mockRejectedValueOnce(new Error('no settings')); // load_settings fails
    mockedInvoke.mockResolvedValueOnce(undefined);                 // open_terminal
    await launchTool('claude', '/my/project');

    expect(mockedInvoke).toHaveBeenCalledWith('open_terminal', {
      workingDir: '/my/project',
      command: 'claude',
    });
  });
});
