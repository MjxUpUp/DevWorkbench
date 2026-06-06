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
    // 使用函数式更新避免陈旧闭包
    let saved: Project[] = [];
    setProjects(prev => {
      saved = [...prev, newProject];
      return saved;
    });
    await invoke('save_projects', { projects: saved });
    return newProject;
  }, []);

  const removeProject = useCallback(async (id: string) => {
    let saved: Project[] = [];
    setProjects(prev => {
      saved = prev.filter(p => p.id !== id);
      return saved;
    });
    await invoke('save_projects', { projects: saved });
  }, []);

  const updateProject = useCallback(async (id: string, patch: Partial<Project>) => {
    let saved: Project[] = [];
    setProjects(prev => {
      saved = prev.map(p => p.id === id ? { ...p, ...patch } : p);
      return saved;
    });
    await invoke('save_projects', { projects: saved });
  }, []);

  const recordOpen = useCallback(async (id: string) => {
    // 后端自行加载/修改/保存，前端只发 ID
    const updated = await invoke<Project[]>('update_project_open', { id });
    setProjects(updated);
  }, []);

  return { projects, loading, error, addProject, removeProject, updateProject, recordOpen, reload: load };
}
