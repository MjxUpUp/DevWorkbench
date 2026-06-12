import { create } from 'zustand';
import type { Project } from '../types';

export type ViewId = 'chat' | 'orchestrate' | 'skill-market' | 'dashboard' | 'settings';

interface NavigationState {
  /** Currently active view in the main stage */
  activeView: ViewId;
  /** Currently selected project (single source of truth) */
  activeProject: Project | null;
  /** Currently selected session ID */
  selectedSessionId: string | null;
  /** Currently expanded project ID in sidebar */
  expandedProjectId: string | null;
  /** Command palette open */
  commandPaletteOpen: boolean;
  /** Add project modal open */
  addProjectOpen: boolean;
  /** Sidebar width (user draggable) */
  sidebarWidth: number;

  setActiveView: (view: ViewId) => void;
  selectProject: (project: Project | null) => void;
  selectSession: (id: string | null) => void;
  toggleProjectExpand: (projectId: string) => void;
  toggleCommandPalette: () => void;
  setCommandPaletteOpen: (open: boolean) => void;
  setAddProjectOpen: (open: boolean) => void;
  setSidebarWidth: (width: number) => void;
}

export const useNavigationStore = create<NavigationState>((set) => ({
  activeView: 'chat',
  activeProject: null,
  selectedSessionId: null,
  expandedProjectId: null,
  commandPaletteOpen: false,
  addProjectOpen: false,
  sidebarWidth: 240,

  setActiveView: (view) => set({ activeView: view }),
  selectProject: (project) => set({ activeProject: project, selectedSessionId: null }),
  selectSession: (id) => set({
    selectedSessionId: id,
    // Switch to chat view when selecting a session; stay on current view when deselecting
    ...(id ? { activeView: 'chat' as ViewId } : {}),
  }),
  toggleProjectExpand: (projectId) => set((s) => ({
    expandedProjectId: s.expandedProjectId === projectId ? null : projectId,
  })),
  toggleCommandPalette: () => set((s) => ({ commandPaletteOpen: !s.commandPaletteOpen })),
  setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
  setAddProjectOpen: (open) => set({ addProjectOpen: open }),
  setSidebarWidth: (width) => set({ sidebarWidth: Math.max(180, Math.min(400, width)) }),
}));
