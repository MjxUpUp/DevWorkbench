import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { Project } from '../types';
import { isTauri } from '../utils/env';

interface ProjectState {
  projects: Project[];
  loading: boolean;
  error: string | null;

  loadProjects: () => Promise<void>;
  addProject: (project: Omit<Project, 'id' | 'open_count' | 'last_opened_at' | 'created_at' | 'starred' | 'last_opened_tools' | 'workspace_tools'>) => Promise<Project>;
  removeProject: (id: string) => Promise<void>;
  updateProject: (id: string, patch: Partial<Project>) => Promise<void>;
  updateProjectOpen: (id: string) => Promise<void>;
  recordToolOpen: (id: string, tool: string) => Promise<void>;
  loadSettings: () => Promise<import('../types').AppSettings>;
  saveSettings: (settings: import('../types').AppSettings) => Promise<void>;
}

export const useProjectStore = create<ProjectState>((set) => ({
  projects: [],
  loading: true,
  error: null,

  loadProjects: async () => {
    // Plain browser / vite preview has no Tauri IPC — leave an empty list
    // instead of surfacing a misleading "加载项目失败" error banner.
    if (!isTauri()) {
      set({ projects: [], loading: false, error: null });
      return;
    }
    try {
      set({ error: null });
      const data = await invoke<Project[]>('load_projects');
      set({ projects: data });
    } catch (e) {
      set({ error: `加载项目失败: ${e}` });
    } finally {
      set({ loading: false });
    }
  },

  addProject: async (project) => {
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
    set({ projects: updated });
    return newProject;
  },

  removeProject: async (id) => {
    await invoke<Project[]>('remove_project', { id });
    set((s) => ({ projects: s.projects.filter(p => p.id !== id) }));
  },

  updateProject: async (id, patch) => {
    const patchJson = Object.fromEntries(
      Object.entries(patch).filter(([_, v]) => v !== undefined)
    );
    const updated = await invoke<Project[]>('update_project', { id, patch: patchJson });
    set({ projects: updated });
  },

  updateProjectOpen: async (id) => {
    const updated = await invoke<Project[]>('update_project_open', { id });
    set({ projects: updated });
  },

  recordToolOpen: async (id, tool) => {
    await invoke('record_tool_open', { id, tool });
  },

  loadSettings: async () => {
    return invoke<import('../types').AppSettings>('load_settings');
  },

  saveSettings: async (settings) => {
    await invoke('save_settings', { settings });
  },
}));
