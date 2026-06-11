import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { KnowledgeEntry } from '../types';

interface KnowledgeState {
  entries: KnowledgeEntry[];
  searchResults: KnowledgeEntry[];
  loading: boolean;

  search: (query: string, limit?: number) => Promise<void>;
  loadForProject: (projectPath: string) => Promise<void>;
  deleteEntry: (id: string) => Promise<void>;
}

export const useKnowledgeStore = create<KnowledgeState>((set) => ({
  entries: [],
  searchResults: [],
  loading: false,

  search: async (query, limit = 20) => {
    set({ loading: true });
    try {
      const results = await invoke<KnowledgeEntry[]>('search_knowledge', { query, limit });
      set({ searchResults: results });
    } catch (e) {
      console.error('Knowledge search failed:', e);
    } finally {
      set({ loading: false });
    }
  },

  loadForProject: async (projectPath) => {
    set({ loading: true });
    try {
      const entries = await invoke<KnowledgeEntry[]>('get_knowledge_for_project', { projectPath });
      set({ entries });
    } catch (e) {
      console.error('Load knowledge failed:', e);
    } finally {
      set({ loading: false });
    }
  },

  deleteEntry: async (id) => {
    await invoke('delete_knowledge_entry', { id });
    set((s) => ({
      entries: s.entries.filter((e) => e.id !== id),
      searchResults: s.searchResults.filter((e) => e.id !== id),
    }));
  },
}));
