import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useTools } from '../useTools';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
const mockedInvoke = vi.mocked(invoke);

const mockTools = [
  { name: 'code', installed: true, path: '/usr/local/bin/code' },
  { name: 'git', installed: false, path: null },
];

describe('useTools', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    // These tests exercise the Tauri IPC path, so simulate the webview where
    // __TAURI_INTERNALS__ is injected; otherwise the isTauri() guard skips invoke.
    // @ts-expect-error — simulating Tauri injection for the IPC code path
    window.__TAURI_INTERNALS__ = { invoke: () => Promise.resolve() };
  });

  afterEach(() => {
    // @ts-expect-error — cleanup the simulated Tauri global
    delete window.__TAURI_INTERNALS__;
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

    expect(result.current.isInstalled('code')).toBe(true);
  });

  it('isInstalled returns false for uninstalled tools', async () => {
    mockedInvoke.mockResolvedValueOnce(mockTools);

    const { result } = renderHook(() => useTools());

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.isInstalled('git')).toBe(false);
  });

  it('isInstalled returns false for unknown tools', async () => {
    mockedInvoke.mockResolvedValueOnce(mockTools);

    const { result } = renderHook(() => useTools());

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.isInstalled('nonexistent')).toBe(false);
  });

  it('skips IPC and surfaces no error in a plain browser (no Tauri)', async () => {
    // @ts-expect-error — simulate plain browser without Tauri IPC
    delete window.__TAURI_INTERNALS__;

    const { result } = renderHook(() => useTools());

    await vi.waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(mockedInvoke).not.toHaveBeenCalled();
    expect(result.current.tools).toEqual([]);
    expect(result.current.error).toBeNull();
  });
});
