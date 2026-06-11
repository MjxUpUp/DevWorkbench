import { create } from 'zustand';
import type { Project } from '../types';

export type TabId = 'overview' | 'sessions' | 'timeline' | 'knowledge';

interface NavigationState {
  /** Currently active tab in the main panel */
  activeTab: TabId;
  /** Currently selected project (single source of truth) */
  activeProject: Project | null;
  /** Currently selected session ID */
  selectedSessionId: string | null;
  /** Currently expanded project ID in sidebar */
  expandedProjectId: string | null;
  /** Command palette open */
  commandPaletteOpen: boolean;
  /** Config center open */
  configCenterOpen: boolean;
  /** Add project modal open */
  addProjectOpen: boolean;
  /** Settings modal open */
  settingsOpen: boolean;

  setActiveTab: (tab: TabId) => void;
  selectProject: (project: Project | null) => void;
  selectSession: (id: string | null) => void;
  toggleProjectExpand: (projectId: string) => void;
  toggleCommandPalette: () => void;
  setCommandPaletteOpen: (open: boolean) => void;
  toggleConfigCenter: () => void;
  setConfigCenterOpen: (open: boolean) => void;
  setAddProjectOpen: (open: boolean) => void;
  setSettingsOpen: (open: boolean) => void;
}

export const useNavigationStore = create<NavigationState>((set) => ({
  activeTab: 'overview',
  activeProject: null,
  selectedSessionId: null,
  expandedProjectId: null,
  commandPaletteOpen: false,
  configCenterOpen: false,
  addProjectOpen: false,
  settingsOpen: false,

  setActiveTab: (tab) => set({ activeTab: tab }),
  selectProject: (project) => set({ activeProject: project, selectedSessionId: null }),
  selectSession: (id) => set({
    selectedSessionId: id,
    // Switch to sessions tab when selecting a session; stay on current tab when deselecting
    ...(id ? { activeTab: 'sessions' as TabId } : {}),
  }),
  toggleProjectExpand: (projectId) => set((s) => ({
    expandedProjectId: s.expandedProjectId === projectId ? null : projectId,
  })),
  toggleCommandPalette: () => set((s) => ({ commandPaletteOpen: !s.commandPaletteOpen })),
  setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
  toggleConfigCenter: () => set((s) => ({ configCenterOpen: !s.configCenterOpen })),
  setConfigCenterOpen: (open) => set({ configCenterOpen: open }),
  setAddProjectOpen: (open) => set({ addProjectOpen: open }),
  setSettingsOpen: (open) => set({ settingsOpen: open }),
}));
