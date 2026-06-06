import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useTools } from '../useTools';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
const mockedInvoke = vi.mocked(invoke);

const mockTools = [
  { name: 'claude', installed: true, path: '/usr/local/bin/claude' },
  { name: 'cursor', installed: false, path: null },
  { name: 'code', installed: true, path: '/usr/local/bin/code' },
];

describe('useTools', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('should load tools on mount', async () => {
    mockedInvoke.mockResolvedValueOnce(mockTools);

    const { result } = renderHook(() => useTools());

    expect(result.current.loading).toBe(true);

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(mockedInvoke).toHaveBeenCalledWith('detect_tools');
    expect(result.current.tools).toEqual(mockTools);
    expect(result.current.error).toBeNull();
  });

  it('should handle detection error', async () => {
    mockedInvoke.mockRejectedValueOnce(new Error('spawn failed'));

    const { result } = renderHook(() => useTools());

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.error).toContain('工具检测失败');
    expect(result.current.tools).toEqual([]);
  });

  it('isInstalled returns true for installed tools', async () => {
    mockedInvoke.mockResolvedValueOnce(mockTools);

    const { result } = renderHook(() => useTools());

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.isInstalled('claude')).toBe(true);
    expect(result.current.isInstalled('code')).toBe(true);
  });

  it('isInstalled returns false for uninstalled tools', async () => {
    mockedInvoke.mockResolvedValueOnce(mockTools);

    const { result } = renderHook(() => useTools());

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.isInstalled('cursor')).toBe(false);
  });

  it('isInstalled returns false for unknown tools', async () => {
    mockedInvoke.mockResolvedValueOnce(mockTools);

    const { result } = renderHook(() => useTools());

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.isInstalled('nonexistent')).toBe(false);
  });
});
