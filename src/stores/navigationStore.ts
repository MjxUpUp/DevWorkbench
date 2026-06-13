import { create } from 'zustand';
import type { Project } from '../types';

export type ViewId = 'task' | 'search' | 'skills' | 'settings';

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
  /** Left column visible — zcode-style single-column toggle (replaces per-view auto-hide) */
  sidebarOpen: boolean;

  setActiveView: (view: ViewId) => void;
  selectProject: (project: Project | null) => void;
  selectSession: (id: string | null) => void;
  toggleProjectExpand: (projectId: string) => void;
  toggleCommandPalette: () => void;
  setCommandPaletteOpen: (open: boolean) => void;
  setAddProjectOpen: (open: boolean) => void;
  setSidebarWidth: (width: number) => void;
  toggleSidebar: () => void;
}

export const useNavigationStore = create<NavigationState>((set) => ({
  activeView: 'task',
  activeProject: null,
  selectedSessionId: null,
  expandedProjectId: null,
  commandPaletteOpen: false,
  addProjectOpen: false,
  sidebarWidth: 240,
  sidebarOpen: true,

  setActiveView: (view) => set({ activeView: view }),
  selectProject: (project) => set({ activeProject: project, selectedSessionId: null }),
  selectSession: (id) => set({
    selectedSessionId: id,
    // Switch to the task view when selecting a session; stay on current view when deselecting
    ...(id ? { activeView: 'task' as ViewId } : {}),
  }),
  toggleProjectExpand: (projectId) => set((s) => ({
    expandedProjectId: s.expandedProjectId === projectId ? null : projectId,
  })),
  toggleCommandPalette: () => set((s) => ({ commandPaletteOpen: !s.commandPaletteOpen })),
  setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
  setAddProjectOpen: (open) => set({ addProjectOpen: open }),
  setSidebarWidth: (width) => set({ sidebarWidth: Math.max(180, Math.min(400, width)) }),
  toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),
}));
