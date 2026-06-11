import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { Project } from '../types';

export function useProjects() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setError(null);
      const data = await invoke<Project[]>('load_projects');
      setProjects(data);
    } catch (e) {
      setError(`加载项目失败: ${e}`);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const addProject = useCallback(async (project: Omit<Project, 'id' | 'open_count' | 'last_opened_at' | 'created_at' | 'starred' | 'last_opened_tools' | 'workspace_tools'>) => {
    const now = new Date().toISOString();
    const newProject: Project = {
      ...project,
      id: crypto.randomUUID(),
      open_count: 0,
      last_opened_at: null,
      starred: false,
      created_at: now,
      last_opened_tools: [],
      workspace_tools: [],
    };
    const updated = await invoke<Project[]>('add_project', { project: newProject });
    setProjects(updated);
    return newProject;
  }, []);

  const updateProject = useCallback(async (id: string, patch: Partial<Project>) => {
    const patchJson = Object.fromEntries(
      Object.entries(patch).filter(([_, v]) => v !== undefined)
    );
    const updated = await invoke<Project[]>('update_project', { id, patch: patchJson });
    setProjects(updated);
  }, []);

  return { projects, loading, error, addProject, updateProject };
}
