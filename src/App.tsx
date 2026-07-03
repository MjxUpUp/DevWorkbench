import { useState, useEffect, Component, type ErrorInfo, type ReactNode } from 'react';
import { AddProject } from './components/AddProject';
import { ActivityBar } from './components/layout/ActivityBar';
import { WorkspaceTabs } from './components/layout/WorkspaceTabs';
import { MainStage } from './components/MainPanel';
import { CommandPalette } from './components/CommandPalette';
import { SettingsView } from './components/settings/SettingsView';
import { OnboardingWizard } from './components/onboarding/OnboardingWizard';
import { TitleBar } from './components/layout/TitleBar';
import { StatusBar } from './components/layout/StatusBar';
import { ToastProvider } from './components/Toast';
import { useTools } from './hooks/useTools';
import { useAgentStore } from './stores/agentStore';
import { useNavigationStore, type ViewId } from './stores/navigationStore';
import { useProjectStore } from './stores/projectStore';
import { useSettingsStore } from './stores/settingsStore';
import { applyTheme } from './utils/theme';
import './styles/index.css';

// Error boundary to catch React render crashes
class ErrorBoundary extends Component<{ children: ReactNode }, { error: Error | null; componentStack: string | null }> {
  state = { error: null as Error | null, componentStack: null as string | null };
  static getDerivedStateFromError(error: Error) { return { error }; }
  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error('[ErrorBoundary]', error, info.componentStack);
    this.setState({ componentStack: info.componentStack ?? null });
  }
  render() {
    if (this.state.error) {
      return (
        <div style={{ padding: 32, color: 'var(--danger)', background: 'var(--bg-canvas)', minHeight: '100vh', fontFamily: 'monospace', whiteSpace: 'pre-wrap', fontSize: 13 }}>
          <h2>React Render Error</h2>
          <p><b>{this.state.error.message}</b></p>
          <p>{this.state.error.stack}</p>
          {this.state.componentStack && (
            <>
              <h3 style={{ marginTop: 16 }}>Component Stack:</h3>
              <p style={{ color: 'var(--text-tertiary)' }}>{this.state.componentStack}</p>
            </>
          )}
        </div>
      );
    }
    return this.props.children;
  }
}

function App() {
  const { error: toolsError } = useTools();

  // Wait for loadSettings to resolve before deciding whether to show the
  // first-run onboarding overlay — otherwise the default (onboarding_completed
  // === false) flashes the wizard for one frame even on an already-onboarded install.
  const [onboardingChecked, setOnboardingChecked] = useState(false);

  // Zustand stores — select only needed fields to avoid re-renders on unrelated state changes
  const addProjectOpen = useNavigationStore((s) => s.addProjectOpen);
  const setAddProjectOpen = useNavigationStore((s) => s.setAddProjectOpen);
  const onboardingOpen = useNavigationStore((s) => s.onboardingOpen);
  const setOnboardingOpen = useNavigationStore((s) => s.setOnboardingOpen);
  const activeView = useNavigationStore((s) => s.activeView);

  // Onboarding overlay trigger: auto-show once on a fresh install
  // (onboarding_completed === false) after settings resolve, OR whenever the
  // user re-opens it from Settings → 引导 (onboardingOpen === true).
  const onboardingCompleted = useSettingsStore((s) => s.settings.onboarding_completed);
  const saveSettings = useSettingsStore((s) => s.saveSettings);

  // Project store — load projects on mount
  const projects = useProjectStore((s) => s.projects);
  const projectError = useProjectStore((s) => s.error);
  const addProject = useProjectStore((s) => s.addProject);
  const loadProjects = useProjectStore((s) => s.loadProjects);

  useEffect(() => { loadProjects(); }, [loadProjects]);

  // Load settings on mount — applies the persisted theme (light/dark/auto) ASAP
  // to avoid a flash of the default theme before the store resolves.
  const loadSettings = useSettingsStore((s) => s.loadSettings);
  useEffect(() => {
    // Apply a sane default immediately; loadSettings will refine it once persisted.
    applyTheme('auto');
    // .then flips onboardingChecked so the wizard overlay doesn't flash on by
    // default for one frame before the persisted onboarding_completed arrives.
    loadSettings().then(() => setOnboardingChecked(true));
  }, [loadSettings]);

  // Initialize agent store event listeners once (use getState to avoid re-render)
  useEffect(() => {
    return useAgentStore.getState().initEventListeners();
  }, []);

  // Session completion notification
  useEffect(() => {
    let permissionRequested = false;
    let unlisten: (() => void) | null = null;

    const requestPermission = async () => {
      if (permissionRequested) return false;
      permissionRequested = true;
      try {
        const { isPermissionGranted, requestPermission: reqPerm } = await import('@tauri-apps/plugin-notification');
        let permitted = await isPermissionGranted();
        if (!permitted) {
          const permission = await reqPerm();
          permitted = permission === 'granted';
        }
        return permitted;
      } catch {
        return false;
      }
    };

    let cancelled = false;
    (async () => {
      const { listen: listenFn } = await import('@tauri-apps/api/event');
      const fn = await listenFn<{ sessionId: string; status: string; exitCode: number | null }>('agent:completed', async (event) => {
        const { status } = event.payload;
        const sessions = useAgentStore.getState().sessions;
        const session = sessions.find(s => s.id === event.payload.sessionId);

        if (!session) return;

        const agentName = session.agentType;
        const title = status === 'completed' ? 'Agent 任务完成' : 'Agent 任务失败';
        const body = `${agentName}: ${session.prompt.slice(0, 80)}${session.prompt.length > 80 ? '...' : ''}`;

        const permitted = await requestPermission();
        if (permitted) {
          try {
            const { sendNotification } = await import('@tauri-apps/plugin-notification');
            sendNotification({ title, body });
          } catch {
            // Notification failed silently
          }
        }
      });
      if (cancelled) {
        fn(); // component unmounted before listen() resolved — clean up now, no leak
      } else {
        unlisten = fn;
      }
    })();

    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  // Global keyboard shortcuts
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Ctrl+N: New conversation — clear the active selection and focus the task
      // view so the user types the first turn. The conversation container is
      // created lazily on send (createConversation), not eagerly here, so we
      // don't spawn a garbage turn with a placeholder prompt.
      if (e.key === 'n' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        if (useNavigationStore.getState().activeProject) {
          useNavigationStore.getState().selectConversation(null);
          useNavigationStore.getState().setActiveView('task');
        }
      }

      // Ctrl+1~3: Switch views (task / trace / settings) — 对齐 ActivityBar 视图图标顺序
      // （search 是命令面板入口，非独立 view，不占 Ctrl 槽位）
      if ((e.ctrlKey || e.metaKey) && e.key >= '1' && e.key <= '3') {
        e.preventDefault();
        const views: ViewId[] = ['task', 'trace', 'settings'];
        const idx = parseInt(e.key) - 1;
        if (idx < views.length) {
          useNavigationStore.getState().setActiveView(views[idx]);
        }
      }
    };

    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, []);

  return (
    <ErrorBoundary>
    <ToastProvider>
    <div className="app">
      <TitleBar />
      <ActivityBar />
      <MainStage />
      <CommandPalette />
      {activeView === 'settings' && <SettingsView />}

      {onboardingChecked && (!onboardingCompleted || onboardingOpen) && (
        <OnboardingWizard
          closable={onboardingCompleted}
          onClose={() => setOnboardingOpen(false)}
          onDone={() => {
            void saveSettings({ onboarding_completed: true });
            setOnboardingOpen(false);
          }}
        />
      )}

      {(projectError || toolsError) && <div className="error-banner" style={{position:'fixed',top:0,left:'50%',transform:'translateX(-50%)',zIndex:300}}>{projectError || toolsError}</div>}

      {addProjectOpen && (
        <AddProject onAdd={async (p) => { await addProject(p); setAddProjectOpen(false); }} onClose={() => setAddProjectOpen(false)} existingProjects={projects} />
      )}

      <WorkspaceTabs />
      <StatusBar />
    </div>
    </ToastProvider>
    </ErrorBoundary>
  );
}

export default App;
