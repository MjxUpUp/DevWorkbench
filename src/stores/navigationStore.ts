import { create } from 'zustand';
import type { Project } from '../types';

export type ViewId = 'task' | 'search' | 'skills' | 'orchestrate' | 'settings';

interface NavigationState {
  /** Currently active view in the main stage */
  activeView: ViewId;
  /** Currently selected project (single source of truth) */
  activeProject: Project | null;
  /** Currently selected conversation ID (the topic container). The main task
   *  view renders the turns of THIS conversation. Selecting a project clears it;
   *  sending the first message creates + selects one. */
  selectedConversationId: string | null;
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
  selectConversation: (id: string | null) => void;
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
  selectedConversationId: null,
  expandedProjectId: null,
  commandPaletteOpen: false,
  addProjectOpen: false,
  sidebarWidth: 240,
  sidebarOpen: true,

  setActiveView: (view) => set({ activeView: view }),
  // Switching project resets the active conversation — a conversation belongs to
  // exactly one project, so the old selection is meaningless in the new scope.
  selectProject: (project) => set({ activeProject: project, selectedConversationId: null }),
  selectConversation: (id) => set({
    selectedConversationId: id,
    // Switch to the task view when selecting a conversation.
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
