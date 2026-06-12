import { useEffect, Component, type ErrorInfo, type ReactNode } from 'react';
import { AddProject } from './components/AddProject';
import { Sidebar } from './components/Sidebar';
import { MainStage } from './components/MainPanel';
import { CommandPalette } from './components/CommandPalette';
import { ActivityBar } from './components/layout/ActivityBar';
import { StatusBar } from './components/layout/StatusBar';
import { ToastProvider } from './components/Toast';
import { useTools } from './hooks/useTools';
import { useAgentStore } from './stores/agentStore';
import { useNavigationStore, type ViewId } from './stores/navigationStore';
import { useProjectStore } from './stores/projectStore';
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
        <div style={{ padding: 32, color: '#ff6b6b', background: '#F7F8FA', minHeight: '100vh', fontFamily: 'monospace', whiteSpace: 'pre-wrap', fontSize: 13 }}>
          <h2>React Render Error</h2>
          <p><b>{this.state.error.message}</b></p>
          <p>{this.state.error.stack}</p>
          {this.state.componentStack && (
            <>
              <h3 style={{ marginTop: 16 }}>Component Stack:</h3>
              <p style={{ color: '#888' }}>{this.state.componentStack}</p>
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

  // Zustand stores — select only needed fields to avoid re-renders on unrelated state changes
  const addProjectOpen = useNavigationStore((s) => s.addProjectOpen);
  const setAddProjectOpen = useNavigationStore((s) => s.setAddProjectOpen);

  // Project store — load projects on mount
  const projects = useProjectStore((s) => s.projects);
  const projectError = useProjectStore((s) => s.error);
  const addProject = useProjectStore((s) => s.addProject);
  const loadProjects = useProjectStore((s) => s.loadProjects);

  useEffect(() => { loadProjects(); }, [loadProjects]);

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

    (async () => {
      const { listen: listenFn } = await import('@tauri-apps/api/event');
      unlisten = await listenFn<{ sessionId: string; status: string; exitCode: number | null }>('agent:completed', async (event) => {
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
    })();

    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  // Global keyboard shortcuts
  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent) => {
      // Ctrl+N: New conversation
      if (e.key === 'n' && (e.ctrlKey || e.metaKey)) {
        e.preventDefault();
        const activeProject = useNavigationStore.getState().activeProject;
        if (activeProject) {
          const agent = useAgentStore.getState().getDefaultAgent();
          if (agent) {
            useAgentStore.getState().newConversation(activeProject.path, '新对话', agent);
          }
        }
      }

      // Ctrl+1~5: Switch views
      if ((e.ctrlKey || e.metaKey) && e.key >= '1' && e.key <= '5') {
        e.preventDefault();
        const views: ViewId[] = ['chat', 'orchestrate', 'skill-market', 'dashboard', 'settings'];
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
      <ActivityBar />
      <Sidebar />
      <MainStage />
      <CommandPalette />

      {(projectError || toolsError) && <div className="error-banner" style={{position:'fixed',top:0,left:'50%',transform:'translateX(-50%)',zIndex:300}}>{projectError || toolsError}</div>}

      {addProjectOpen && (
        <AddProject onAdd={async (p) => { await addProject(p); setAddProjectOpen(false); }} onClose={() => setAddProjectOpen(false)} existingProjects={projects} />
      )}

      <StatusBar />
    </div>
    </ToastProvider>
    </ErrorBoundary>
  );
}

export default App;
