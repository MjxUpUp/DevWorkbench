import { describe, it, expect, vi, beforeEach } from 'vitest';
import { renderHook, act, waitFor } from '@testing-library/react';
import { useProjects } from '../useProjects';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

import { invoke } from '@tauri-apps/api/core';
const mockedInvoke = vi.mocked(invoke);

const mockProjects = [
  {
    id: '1',
    name: 'Project A',
    description: 'Test project A',
    path: '/path/a',
    tags: ['react'],
    cover_image: null,
    open_count: 5,
    last_opened_at: '2025-01-01T00:00:00.000Z',
    starred: false,
    created_at: '2024-01-01T00:00:00.000Z',
  },
  {
    id: '2',
    name: 'Project B',
    description: 'Test project B',
    path: '/path/b',
    tags: ['rust'],
    cover_image: null,
    open_count: 0,
    last_opened_at: null,
    starred: true,
    created_at: '2024-06-01T00:00:00.000Z',
  },
];

describe('useProjects', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('should load projects on mount', async () => {
    mockedInvoke.mockResolvedValueOnce(mockProjects);

    const { result } = renderHook(() => useProjects());

    expect(result.current.loading).toBe(true);

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(mockedInvoke).toHaveBeenCalledWith('load_projects');
    expect(result.current.projects).toEqual(mockProjects);
    expect(result.current.error).toBeNull();
  });

  it('should handle load error', async () => {
    mockedInvoke.mockRejectedValueOnce(new Error('disk error'));

    const { result } = renderHook(() => useProjects());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    expect(result.current.error).toContain('加载项目失败');
    expect(result.current.projects).toEqual([]);
  });

  it('should add a project', async () => {
    mockedInvoke.mockResolvedValueOnce(mockProjects);
    mockedInvoke.mockResolvedValueOnce(undefined);

    const { result } = renderHook(() => useProjects());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    const input = {
      name: 'New Project',
      description: 'A new one',
      path: '/path/new',
      tags: ['typescript'],
      cover_image: null as string | null,
    };

    await act(async () => {
      await result.current.addProject(input);
    });

    expect(result.current.projects).toHaveLength(3);
    const added = result.current.projects.find(p => p.name === 'New Project');
    expect(added).toBeDefined();
    expect(added!.path).toBe('/path/new');
    expect(added!.starred).toBe(false);
    expect(added!.open_count).toBe(0);

    expect(mockedInvoke).toHaveBeenCalledWith('save_projects', expect.anything());
  });

  it('should remove a project', async () => {
    mockedInvoke.mockResolvedValueOnce(mockProjects);
    mockedInvoke.mockResolvedValueOnce(undefined);

    const { result } = renderHook(() => useProjects());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    await act(async () => {
      await result.current.removeProject('1');
    });

    expect(result.current.projects).toHaveLength(1);
    expect(result.current.projects[0].id).toBe('2');
    expect(mockedInvoke).toHaveBeenCalledWith('save_projects', expect.anything());
  });

  it('should update a project', async () => {
    mockedInvoke.mockResolvedValueOnce(mockProjects);
    mockedInvoke.mockResolvedValueOnce(undefined);

    const { result } = renderHook(() => useProjects());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    await act(async () => {
      await result.current.updateProject('1', { name: 'Updated A' });
    });

    const updated = result.current.projects.find(p => p.id === '1');
    expect(updated!.name).toBe('Updated A');
    expect(mockedInvoke).toHaveBeenCalledWith('save_projects', expect.anything());
  });

  it('should toggle star via updateProject', async () => {
    mockedInvoke.mockResolvedValueOnce(mockProjects);
    mockedInvoke.mockResolvedValueOnce(undefined);

    const { result } = renderHook(() => useProjects());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    await act(async () => {
      await result.current.updateProject('1', { starred: true });
    });

    const updated = result.current.projects.find(p => p.id === '1');
    expect(updated!.starred).toBe(true);
  });

  it('should record open and update projects from backend', async () => {
    const updatedProjects = mockProjects.map(p =>
      p.id === '1' ? { ...p, open_count: 6, last_opened_at: '2025-06-06T00:00:00.000Z' } : p
    );
    mockedInvoke.mockResolvedValueOnce(mockProjects);
    mockedInvoke.mockResolvedValueOnce(updatedProjects);

    const { result } = renderHook(() => useProjects());

    await waitFor(() => {
      expect(result.current.loading).toBe(false);
    });

    await act(async () => {
      await result.current.recordOpen('1');
    });

    expect(mockedInvoke).toHaveBeenCalledWith('update_project_open', { id: '1' });
    expect(result.current.projects.find(p => p.id === '1')!.open_count).toBe(6);
  });
});
