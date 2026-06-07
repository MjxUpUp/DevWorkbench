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
    // 后端原子操作：load → push → save → 返回完整数组
    const updated = await invoke<Project[]>('add_project', { project: newProject });
    setProjects(updated);
    return newProject;
  }, []);

  const removeProject = useCallback(async (id: string) => {
    // 后端原子操作：load → retain → save → 返回完整数组
    const updated = await invoke<Project[]>('remove_project', { id });
    setProjects(updated);
  }, []);

  const updateProject = useCallback(async (id: string, patch: Partial<Project>) => {
    // 后端原子操作：load → patch → save → 返回完整数组
    // 前端 camelCase 字段名转 snake_case 由 Tauri 自动处理
    const patchJson = Object.fromEntries(
      Object.entries(patch).filter(([_, v]) => v !== undefined)
    );
    const updated = await invoke<Project[]>('update_project', { id, patch: patchJson });
    setProjects(updated);
  }, []);

  const recordOpen = useCallback(async (id: string) => {
    const updated = await invoke<Project[]>('update_project_open', { id });
    setProjects(updated);
  }, []);

  const recordToolOpen = useCallback(async (id: string, toolName: string) => {
    try {
      const updated = await invoke<Project[]>('record_tool_open', { id, toolName });
      setProjects(updated);
    } catch (e) {
      console.warn('record_tool_open failed:', e);
    }
  }, []);

  return { projects, loading, error, addProject, removeProject, updateProject, recordOpen, recordToolOpen, reload: load };
}
