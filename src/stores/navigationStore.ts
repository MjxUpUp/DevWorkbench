import { create } from 'zustand';
import type { Project } from '../types';
import type { SettingsSection } from '../components/settings/types';

export type ViewId = 'task' | 'search' | 'settings' | 'trace';

interface NavigationState {
  /** Currently active view in the main stage */
  activeView: ViewId;
  /** Currently selected project (single source of truth). UI 称「工作区」，
   *  标识符保留 project* 避免 DB/IPC 层 churn。 */
  activeProject: Project | null;
  /** Currently selected conversation ID (the topic container). The main task
   *  view renders the turns of THIS conversation. Selecting a project clears it;
   *  sending the first message creates + selects one. */
  selectedConversationId: string | null;
  /** Command palette open */
  commandPaletteOpen: boolean;
  /** Add project modal open */
  addProjectOpen: boolean;
  /** Onboarding wizard overlay open (relaunch from Settings). The first-run
   *  trigger is `settings.onboarding_completed === false`, not this flag; this
   *  only re-opens an already-completed wizard when the user clicks the relaunch
   *  button in Settings → 引导. */
  onboardingOpen: boolean;
  /** The session whose LLM traces the 'trace' view shows. Set by AgentMessage's
   *  「🔍 Trace」 button; cleared when leaving the trace view. null = no session
   *  selected → TraceView shows its empty state. */
  traceSessionId: string | null;
  /** 进设置页时默认展开的分区。技能目录已下放到设置页统一管理，外部入口（命令面板
   *  「技能」）跳设置页时设为 'skills' 直达该分区；SettingsView 读取后清空，避免下次从
   *  用户菜单进设置仍落在该分区。null = 用默认分区。类型为 SettingsSection（type-only
   *  import 自 ../components/settings/types，编译期擦除，运行时无循环依赖）。 */
  settingsInitialSection: SettingsSection | null;

  setActiveView: (view: ViewId) => void;
  selectProject: (project: Project | null) => void;
  selectConversation: (id: string | null) => void;
  toggleCommandPalette: () => void;
  setCommandPaletteOpen: (open: boolean) => void;
  setAddProjectOpen: (open: boolean) => void;
  setOnboardingOpen: (open: boolean) => void;
  /** Jump to the trace view scoped to one session (a turn). The trace view then
   *  fetches that session's LLM HTTP calls via traceStore. */
  setTrace: (sessionId: string) => void;
  setSettingsInitialSection: (section: SettingsSection | null) => void;
}

export const useNavigationStore = create<NavigationState>((set) => ({
  activeView: 'task',
  activeProject: null,
  selectedConversationId: null,
  commandPaletteOpen: false,
  addProjectOpen: false,
  onboardingOpen: false,
  traceSessionId: null,
  settingsInitialSection: null,

  setActiveView: (view) => set({ activeView: view }),
  // Switching project resets the active conversation — a conversation belongs to
  // exactly one project, so the old selection is meaningless in the new scope.
  selectProject: (project) => set({ activeProject: project, selectedConversationId: null }),
  selectConversation: (id) => set({
    selectedConversationId: id,
    // Switch to the task view when selecting a conversation.
    ...(id ? { activeView: 'task' as ViewId } : {}),
  }),
  toggleCommandPalette: () => set((s) => ({ commandPaletteOpen: !s.commandPaletteOpen })),
  setCommandPaletteOpen: (open) => set({ commandPaletteOpen: open }),
  setAddProjectOpen: (open) => set({ addProjectOpen: open }),
  setOnboardingOpen: (open) => set({ onboardingOpen: open }),
  setTrace: (sessionId) => set({ traceSessionId: sessionId, activeView: 'trace' }),
  setSettingsInitialSection: (section) => set({ settingsInitialSection: section }),
}));
