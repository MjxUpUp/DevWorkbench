import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { Project } from '../types';

export function useProjects() {
  const [projects, setProjects] = useState<Project[]>([]);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    try {
      const data = await invoke<Project[]>('load_projects');
      setProjects(data);
    } catch (e) {
      console.error('加载项目失败:', e);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const save = useCallback(async (updated: Project[]) => {
    await invoke('save_projects', { projects: updated });
    setProjects(updated);
  }, []);

  const addProject = useCallback(async (project: Omit<Project, 'id' | 'open_count' | 'last_opened_at' | 'created_at' | 'starred'>) => {
    const now = new Date().toISOString();
    const newProject: Project = {
      ...project,
      id: crypto.randomUUID(),
      open_count: 0,
      last_opened_at: null,
      starred: false,
      created_at: now,
    };
    const updated = [...projects, newProject];
    await save(updated);
    return newProject;
  }, [projects, save]);

  const removeProject = useCallback(async (id: string) => {
    const updated = projects.filter(p => p.id !== id);
    await save(updated);
  }, [projects, save]);

  const updateProject = useCallback(async (id: string, patch: Partial<Project>) => {
    const updated = projects.map(p => p.id === id ? { ...p, ...patch } : p);
    await save(updated);
  }, [projects, save]);

  const recordOpen = useCallback(async (id: string) => {
    const updated = await invoke<Project[]>('update_project_open', { id, projects });
    setProjects(updated);
  }, [projects]);

  return { projects, loading, addProject, removeProject, updateProject, recordOpen, reload: load };
}
