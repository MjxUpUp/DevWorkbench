import { describe, it, expect, vi, beforeEach } from 'vitest';
import { launchTool } from '../launchTool';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
const mockedInvoke = vi.mocked(invoke);

describe('launchTool', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('launches claude via open_terminal with command', async () => {
    mockedInvoke.mockResolvedValueOnce(undefined);
    await launchTool('claude', '/my/project');

    expect(mockedInvoke).toHaveBeenCalledWith('open_terminal', {
      workingDir: '/my/project',
      command: 'claude',
    });
  });

  it('launches terminal via open_terminal without command', async () => {
    mockedInvoke.mockResolvedValueOnce(undefined);
    await launchTool('terminal', '/my/project');

    expect(mockedInvoke).toHaveBeenCalledWith('open_terminal', {
      workingDir: '/my/project',
    });
  });

  it('launches cursor via open_in_editor', async () => {
    mockedInvoke.mockResolvedValueOnce(undefined);
    await launchTool('cursor', '/my/project');

    expect(mockedInvoke).toHaveBeenCalledWith('open_in_editor', {
      editor: 'cursor',
      projectPath: '/my/project',
    });
  });

  it('launches code via open_in_editor', async () => {
    mockedInvoke.mockResolvedValueOnce(undefined);
    await launchTool('code', '/my/project');

    expect(mockedInvoke).toHaveBeenCalledWith('open_in_editor', {
      editor: 'code',
      projectPath: '/my/project',
    });
  });

  it('launches finder via open_in_finder', async () => {
    mockedInvoke.mockResolvedValueOnce(undefined);
    await launchTool('finder', '/my/project');

    expect(mockedInvoke).toHaveBeenCalledWith('open_in_finder', {
      path: '/my/project',
    });
  });

  it('throws for unknown tool', async () => {
    await expect(launchTool('unknown-tool', '/path')).rejects.toThrow('未知工具: unknown-tool');
    expect(mockedInvoke).not.toHaveBeenCalled();
  });
});
