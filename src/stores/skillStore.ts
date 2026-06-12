import { create } from 'zustand';

export interface SkillItem {
  id: string;
  name: string;
  org: string;
  description: string;
  icon: string;
  version: string;
  qualityScore: number; // 0-100
  securityScore: number; // 0-100
  qualityLevel: 1 | 2 | 3 | 4;
  securityLevel: 1 | 2 | 3 | 4;
  installs: number;
  rating: number; // 1-5
  category: 'orchestration' | 'quality' | 'security' | 'efficiency';
  installed: boolean;
  author?: string;
  compatibleAgents?: string[];
  qualityDetails?: {
    l1: number; // structural compliance
    l2: number; // code quality
    l3: number; // semantic alignment
    l4: number; // behavioral verification
  };
  securityDetails?: {
    l1: number; // identity trust
    l2: number; // static analysis
    l3: number; // dynamic analysis
    l4: number; // permission model
  };
  configSchema?: Record<string, unknown>;
}

export type SkillCategory = SkillItem['category'] | 'all';
export type SkillSort = 'newest' | 'rating' | 'installs';

// Mock data — will be replaced by API calls in Phase 3
const MOCK_SKILLS: SkillItem[] = [
  {
    id: 'sk-001',
    name: 'Forge Pipeline',
    org: 'Dev Workbench',
    description: 'Automated quality pipeline with tiered verification, assertion checks, and compilation gates.',
    icon: '🔥',
    version: '1.2.0',
    qualityScore: 92,
    securityScore: 88,
    qualityLevel: 4,
    securityLevel: 4,
    installs: 3420,
    rating: 4.8,
    category: 'quality',
    installed: true,
    author: 'core-team',
    compatibleAgents: ['claude-opus', 'claude-sonnet', 'gpt-4o'],
    qualityDetails: { l1: 95, l2: 92, l3: 88, l4: 93 },
    securityDetails: { l1: 90, l2: 85, l3: 88, l4: 89 },
  },
  {
    id: 'sk-002',
    name: 'Security Scanner',
    org: 'SecOps Lab',
    description: 'Deep vulnerability scanning with SAST, dependency audit, and permission boundary analysis.',
    icon: '🛡️',
    version: '2.0.1',
    qualityScore: 85,
    securityScore: 96,
    qualityLevel: 3,
    securityLevel: 4,
    installs: 2180,
    rating: 4.6,
    category: 'security',
    installed: false,
    author: 'secops-lab',
    compatibleAgents: ['claude-opus', 'claude-sonnet'],
    qualityDetails: { l1: 88, l2: 85, l3: 82, l4: 85 },
    securityDetails: { l1: 95, l2: 98, l3: 94, l4: 97 },
  },
  {
    id: 'sk-003',
    name: 'DAG Orchestrator',
    org: 'FlowCraft',
    description: 'Visual DAG workflow builder with parallel execution, conditional gates, and retry policies.',
    icon: '⚡',
    version: '1.5.3',
    qualityScore: 78,
    securityScore: 72,
    qualityLevel: 3,
    securityLevel: 3,
    installs: 1560,
    rating: 4.3,
    category: 'orchestration',
    installed: false,
    author: 'flowcraft',
    compatibleAgents: ['claude-opus', 'gpt-4o', 'gemini-pro'],
    qualityDetails: { l1: 82, l2: 78, l3: 75, l4: 77 },
    securityDetails: { l1: 78, l2: 70, l3: 72, l4: 68 },
  },
  {
    id: 'sk-004',
    name: 'Code Reviewer',
    org: 'Dev Workbench',
    description: 'Automated code review with bug detection, style enforcement, and reuse suggestions.',
    icon: '🔍',
    version: '1.1.0',
    qualityScore: 90,
    securityScore: 82,
    qualityLevel: 4,
    securityLevel: 3,
    installs: 4210,
    rating: 4.9,
    category: 'quality',
    installed: true,
    author: 'core-team',
    compatibleAgents: ['claude-opus', 'claude-sonnet', 'gpt-4o'],
    qualityDetails: { l1: 93, l2: 90, l3: 88, l4: 89 },
    securityDetails: { l1: 85, l2: 82, l3: 80, l4: 81 },
  },
  {
    id: 'sk-005',
    name: 'Cache Optimizer',
    org: 'PerfLab',
    description: 'Intelligent caching layer for repeated LLM calls with TTL management and cost tracking.',
    icon: '💾',
    version: '0.9.2',
    qualityScore: 70,
    securityScore: 65,
    qualityLevel: 2,
    securityLevel: 2,
    installs: 890,
    rating: 4.0,
    category: 'efficiency',
    installed: false,
    author: 'perflab',
    compatibleAgents: ['claude-opus', 'claude-sonnet'],
    qualityDetails: { l1: 75, l2: 70, l3: 68, l4: 67 },
    securityDetails: { l1: 70, l2: 65, l3: 62, l4: 63 },
  },
  {
    id: 'sk-006',
    name: 'Permission Guard',
    org: 'SecOps Lab',
    description: 'Fine-grained permission control with scope isolation, audit logging, and policy enforcement.',
    icon: '🔐',
    version: '1.3.1',
    qualityScore: 82,
    securityScore: 94,
    qualityLevel: 3,
    securityLevel: 4,
    installs: 1830,
    rating: 4.5,
    category: 'security',
    installed: false,
    author: 'secops-lab',
    compatibleAgents: ['claude-opus', 'gpt-4o'],
    qualityDetails: { l1: 85, l2: 82, l3: 80, l4: 81 },
    securityDetails: { l1: 92, l2: 95, l3: 94, l4: 95 },
  },
  {
    id: 'sk-007',
    name: 'Parallel Executor',
    org: 'FlowCraft',
    description: 'Run multiple agent tasks concurrently with smart scheduling and result aggregation.',
    icon: '🔄',
    version: '1.0.4',
    qualityScore: 75,
    securityScore: 70,
    qualityLevel: 3,
    securityLevel: 2,
    installs: 1120,
    rating: 4.2,
    category: 'orchestration',
    installed: false,
    author: 'flowcraft',
    compatibleAgents: ['claude-opus', 'claude-sonnet', 'gpt-4o', 'gemini-pro'],
    qualityDetails: { l1: 80, l2: 75, l3: 72, l4: 73 },
    securityDetails: { l1: 75, l2: 68, l3: 70, l4: 67 },
  },
  {
    id: 'sk-008',
    name: 'Token Saver',
    org: 'PerfLab',
    description: 'Reduce token usage through smart prompt compression, context pruning, and response caching.',
    icon: '✂️',
    version: '0.8.0',
    qualityScore: 68,
    securityScore: 60,
    qualityLevel: 2,
    securityLevel: 2,
    installs: 2050,
    rating: 4.1,
    category: 'efficiency',
    installed: false,
    author: 'perflab',
    compatibleAgents: ['claude-opus', 'claude-sonnet', 'gpt-4o'],
    qualityDetails: { l1: 72, l2: 68, l3: 65, l4: 67 },
    securityDetails: { l1: 65, l2: 60, l3: 58, l4: 57 },
  },
  {
    id: 'sk-009',
    name: 'Test Architect',
    org: 'Dev Workbench',
    description: 'Generate integration and E2E test scaffolds from code diffs with smart fixture generation.',
    icon: '🧪',
    version: '1.4.0',
    qualityScore: 88,
    securityScore: 78,
    qualityLevel: 4,
    securityLevel: 3,
    installs: 2670,
    rating: 4.7,
    category: 'quality',
    installed: false,
    author: 'core-team',
    compatibleAgents: ['claude-opus', 'claude-sonnet'],
    qualityDetails: { l1: 90, l2: 88, l3: 86, l4: 88 },
    securityDetails: { l1: 82, l2: 78, l3: 76, l4: 76 },
  },
];

interface SkillState {
  skills: SkillItem[];
  searchQuery: string;
  selectedCategory: SkillCategory;
  selectedSkill: SkillItem | null;
  sortBy: SkillSort;
  loading: boolean;

  search: (query: string) => void;
  setCategory: (category: SkillCategory) => void;
  setSortBy: (sort: SkillSort) => void;
  selectSkill: (skill: SkillItem | null) => void;
  installSkill: (id: string) => void;
  uninstallSkill: (id: string) => void;
}

export const useSkillStore = create<SkillState>((set) => ({
  skills: MOCK_SKILLS,
  searchQuery: '',
  selectedCategory: 'all',
  selectedSkill: null,
  sortBy: 'rating',
  loading: false,

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

  installSkill: (id) => {
    set((s) => ({
      skills: s.skills.map((sk) =>
        sk.id === id ? { ...sk, installed: true, installs: sk.installs + 1 } : sk
      ),
      selectedSkill:
        s.selectedSkill?.id === id
          ? { ...s.selectedSkill, installed: true, installs: s.selectedSkill.installs + 1 }
          : s.selectedSkill,
    }));
  },

  uninstallSkill: (id) => {
    set((s) => ({
      skills: s.skills.map((sk) =>
        sk.id === id ? { ...sk, installed: false, installs: Math.max(0, sk.installs - 1) } : sk
      ),
      selectedSkill:
        s.selectedSkill?.id === id
          ? { ...s.selectedSkill, installed: false, installs: Math.max(0, s.selectedSkill.installs - 1) }
          : s.selectedSkill,
    }));
  },
}));

/** Selector: filtered and sorted skill list */
export function useFilteredSkills(): SkillItem[] {
  const skills = useSkillStore((s) => s.skills);
  const query = useSkillStore((s) => s.searchQuery);
  const category = useSkillStore((s) => s.selectedCategory);
  const sortBy = useSkillStore((s) => s.sortBy);

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
}
