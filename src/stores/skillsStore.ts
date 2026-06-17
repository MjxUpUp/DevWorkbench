import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { Skill, SkillCatalogEntry } from '../types';

/**
 * Store for the skills registry (the `skills` SQLite table) + the on-disk skill
 * catalog. The kernel loads SKILL.md files from ~/.dev-workbench/skills (and
 * project-local .agents/skills) into ToolRegistry at agent build time; this
 * store exposes the registered metadata + a browse/install/uninstall surface
 * for the Settings → 技能 page, backed by the already-registered Rust commands
 * `list_skills` / `skill_catalog` / `install_skill_from_catalog` / `uninstall_skill`.
 */
interface SkillsState {
  installed: Skill[];
  catalog: SkillCatalogEntry[];
  loading: boolean;
  loadInstalled: () => Promise<void>;
  loadCatalog: (projectPath?: string) => Promise<void>;
  installFromCatalog: (name: string, source: string) => Promise<Skill>;
  uninstall: (id: string) => Promise<void>;
}

export const useSkillsStore = create<SkillsState>((set) => ({
  installed: [],
  catalog: [],
  loading: false,

  loadInstalled: async () => {
    set({ loading: true });
    try {
      const installed = await invoke<Skill[]>('list_skills');
      set({ installed });
    } catch (e) {
      console.error('list_skills failed:', e);
    } finally {
      set({ loading: false });
    }
  },

  loadCatalog: async (projectPath?: string) => {
    try {
      const catalog = await invoke<SkillCatalogEntry[]>(
        'skill_catalog',
        projectPath ? { projectPath } : {},
      );
      set({ catalog });
    } catch (e) {
      console.error('skill_catalog failed:', e);
    }
  },

  installFromCatalog: async (name, source) => {
    const skill = await invoke<Skill>('install_skill_from_catalog', { name, source });
    // Merge: replace any same-id entry, else append.
    set((s) => ({
      installed: [...s.installed.filter((x) => x.id !== skill.id), skill],
    }));
    return skill;
  },

  uninstall: async (id) => {
    await invoke('uninstall_skill', { id });
    set((s) => ({ installed: s.installed.filter((x) => x.id !== id) }));
  },
}));
