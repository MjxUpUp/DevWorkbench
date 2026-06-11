import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { ActivityEvent } from '../types';

interface ActivityState {
  events: ActivityEvent[];
  loading: boolean;

  loadForProject: (projectPath: string) => Promise<void>;
  loadRecent: (limit?: number) => Promise<void>;
}

export const useActivityStore = create<ActivityState>((set) => ({
  events: [],
  loading: false,

  loadForProject: async (projectPath) => {
    set({ loading: true });
    try {
      const events = await invoke<ActivityEvent[]>('get_project_activity', { projectPath });
      set({ events });
    } catch (e) {
      console.error('Load activity failed:', e);
    } finally {
      set({ loading: false });
    }
  },

  loadRecent: async (limit = 50) => {
    set({ loading: true });
    try {
      const events = await invoke<ActivityEvent[]>('get_recent_activity', { limit });
      set({ events });
    } catch (e) {
      console.error('Load recent activity failed:', e);
    } finally {
      set({ loading: false });
    }
  },
}));
