import { create } from 'zustand';
import { useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';

export interface SkillItem {
  id: string;
  name: string;
  org: string;
  description: string;
  icon: string;
  version: string;
  qualityScore: number;
  securityScore: number;
  qualityLevel: 1 | 2 | 3 | 4;
  securityLevel: 1 | 2 | 3 | 4;
  installs: number;
  rating: number;
  category: 'orchestration' | 'quality' | 'security' | 'efficiency';
  installed: boolean;
  author?: string;
  compatibleAgents?: string[];
  qualityDetails?: {
    l1: number;
    l2: number;
    l3: number;
    l4: number;
  };
  securityDetails?: {
    l1: number;
    l2: number;
    l3: number;
    l4: number;
  };
  configSchema?: Record<string, unknown>;
}

export type SkillCategory = SkillItem['category'] | 'all';
export type SkillSort = 'newest' | 'rating' | 'installs';

/** Shared category → CSS color variable mapping */
export const CATEGORY_COLOR_MAP: Record<string, string> = {
  orchestration: 'var(--skill-orchestration)',
  quality: 'var(--skill-quality)',
  security: 'var(--skill-security)',
  efficiency: 'var(--skill-efficiency)',
};

interface SkillState {
  skills: SkillItem[];
  searchQuery: string;
  selectedCategory: SkillCategory;
  selectedSkill: SkillItem | null;
  sortBy: SkillSort;
  loading: boolean;

  fetchSkills: () => Promise<void>;
  search: (query: string) => void;
  setCategory: (category: SkillCategory) => void;
  setSortBy: (sort: SkillSort) => void;
  selectSkill: (skill: SkillItem | null) => void;
  installSkill: (id: string) => Promise<void>;
  uninstallSkill: (id: string) => Promise<void>;
}

/** Fetch bundled skills catalog from public/ */
async function fetchCatalog(): Promise<SkillItem[]> {
  try {
    const resp = await fetch('/skills-catalog.json');
    if (!resp.ok) return [];
    return await resp.json();
  } catch {
    return [];
  }
}

/** Fetch installed skill IDs from backend */
async function fetchInstalledIds(): Promise<Set<string>> {
  try {
    type BackendSkill = { id: string };
    const installed = await invoke<BackendSkill[]>('list_skills');
    return new Set(installed.map(s => s.id));
  } catch {
    return new Set();
  }
}

export const useSkillStore = create<SkillState>((set, get) => ({
  skills: [],
  searchQuery: '',
  selectedCategory: 'all',
  selectedSkill: null,
  sortBy: 'rating',
  loading: false,

  fetchSkills: async () => {
    set({ loading: true });
    try {
      const [catalog, installedIds] = await Promise.all([fetchCatalog(), fetchInstalledIds()]);

      // Merge: catalog skills get installed flag from backend
      const skills: SkillItem[] = catalog.map(s => ({
        ...s,
        installed: installedIds.has(s.id),
      }));

      set({ skills, loading: false });
    } catch (e) {
      console.error('Failed to fetch skills:', e);
      set({ loading: false });
    }
  },

  search: (query) => {
    set({ searchQuery: query });
  },

  setCategory: (category) => {
    set({ selectedCategory: category });
  },

  setSortBy: (sort) => {
    set({ sortBy: sort });
  },

  selectSkill: (skill) => {
    set({ selectedSkill: skill });
  },

  installSkill: async (id) => {
    const skill = get().skills.find(s => s.id === id);
    if (!skill) return;

    try {
      // Build backend Skill object — catalog metadata goes into metadata field
      const metadata = JSON.stringify({
        description: skill.description,
        icon: skill.icon,
        category: skill.category,
        securityScore: skill.securityScore,
        installs: skill.installs,
        rating: skill.rating,
        author: skill.author,
        compatibleAgents: skill.compatibleAgents ? JSON.stringify(skill.compatibleAgents) : undefined,
        qualityDetails: skill.qualityDetails ? JSON.stringify(skill.qualityDetails) : undefined,
        securityDetails: skill.securityDetails ? JSON.stringify(skill.securityDetails) : undefined,
      });

      await invoke('install_skill', {
        skill: {
          id: skill.id,
          org: skill.org,
          name: skill.name,
          version: skill.version,
          installedAt: new Date().toISOString(),
          path: null,
          qualityScore: skill.qualityScore,
          metadata,
        },
      });

      // Update local state
      set((s) => ({
        skills: s.skills.map((sk) =>
          sk.id === id ? { ...sk, installed: true, installs: sk.installs + 1 } : sk
        ),
        selectedSkill:
          s.selectedSkill?.id === id
            ? { ...s.selectedSkill, installed: true, installs: s.selectedSkill.installs + 1 }
            : s.selectedSkill,
      }));
    } catch (e) {
      console.error('Failed to install skill:', e);
    }
  },

  uninstallSkill: async (id) => {
    try {
      await invoke('uninstall_skill', { id });

      set((s) => ({
        skills: s.skills.map((sk) =>
          sk.id === id ? { ...sk, installed: false, installs: Math.max(0, sk.installs - 1) } : sk
        ),
        selectedSkill:
          s.selectedSkill?.id === id
            ? { ...s.selectedSkill, installed: false, installs: Math.max(0, s.selectedSkill.installs - 1) }
            : s.selectedSkill,
      }));
    } catch (e) {
      console.error('Failed to uninstall skill:', e);
    }
  },
}));

/** Selector: filtered and sorted skill list (memoized) */
export function useFilteredSkills(): SkillItem[] {
  const skills = useSkillStore((s) => s.skills);
  const query = useSkillStore((s) => s.searchQuery);
  const category = useSkillStore((s) => s.selectedCategory);
  const sortBy = useSkillStore((s) => s.sortBy);

  return useMemo(() => {
    let filtered = skills;

    if (category !== 'all') {
      filtered = filtered.filter((s) => s.category === category);
    }

    if (query.trim()) {
      const q = query.toLowerCase();
      filtered = filtered.filter(
        (s) =>
          s.name.toLowerCase().includes(q) ||
          s.description.toLowerCase().includes(q) ||
          s.org.toLowerCase().includes(q)
      );
    }

    return [...filtered].sort((a, b) => {
      switch (sortBy) {
        case 'rating':
          return b.rating - a.rating;
        case 'installs':
          return b.installs - a.installs;
        case 'newest':
        default:
          return 0;
      }
    });
  }, [skills, query, category, sortBy]);
}
