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

export const useKnowledgeStore = create<KnowledgeState>((set) => {
  // Monotonic request ids per operation: the CommandPalette debounces search,
  // but debounce alone doesn't prevent the race once two searches are both
  // in-flight (slow backend) — the older one can resolve last and clobber the
  // newer results. Same hazard for loadForProject on rapid project switching.
  let searchSeq = 0;
  let loadSeq = 0;
  return {
  entries: [],
  searchResults: [],
  loading: false,

  search: async (query, limit = 20) => {
    const myId = ++searchSeq;
    set({ loading: true });
    try {
      const results = await invoke<KnowledgeEntry[]>('search_knowledge', { query, limit });
      if (myId !== searchSeq) return; // superseded by a newer query — drop stale results
      set({ searchResults: results });
    } catch (e) {
      console.error('Knowledge search failed:', e);
    } finally {
      if (myId === searchSeq) set({ loading: false });
    }
  },

  loadForProject: async (projectPath) => {
    const myId = ++loadSeq;
    set({ loading: true });
    try {
      const entries = await invoke<KnowledgeEntry[]>('get_knowledge_for_project', { projectPath });
      if (myId !== loadSeq) return;
      set({ entries });
    } catch (e) {
      console.error('Load knowledge failed:', e);
    } finally {
      if (myId === loadSeq) set({ loading: false });
    }
  },

  deleteEntry: async (id) => {
    await invoke('delete_knowledge_entry', { id });
    set((s) => ({
      entries: s.entries.filter((e) => e.id !== id),
      searchResults: s.searchResults.filter((e) => e.id !== id),
    }));
  },
};
});
