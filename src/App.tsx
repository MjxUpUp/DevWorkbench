import { useEffect, Component, type ErrorInfo, type ReactNode } from 'react';
import { AddProject } from './components/AddProject';
import { Settings } from './components/Settings';
import { Sidebar } from './components/Sidebar';
import { MainPanel } from './components/MainPanel';
import { CommandPalette } from './components/CommandPalette';
import { ConfigCenter } from './components/ConfigCenter';
import { ToastProvider } from './components/Toast';
import { useTools } from './hooks/useTools';
import { useAgentStore } from './stores/agentStore';
import { useNavigationStore } from './stores/navigationStore';
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
        <div style={{ padding: 32, color: '#ff6b6b', background: '#1a1a2e', minHeight: '100vh', fontFamily: 'monospace', whiteSpace: 'pre-wrap', fontSize: 13 }}>
          <h2>React Render Error</h2>
          <p><b>{this.state.error.message}</b></p>
          <p>{this.state.error.stack}</p>
          {this.state.componentStack && (
            <>
              <h3 style={{ marginTop: 16 }}>Component Stack:</h3>
              <p style={{ color: '#aaa' }}>{this.state.componentStack}</p>
            </>
          )}
        </div>
      );
    }
    return this.props.children;
  }
}

function App() {
  const { tools, error: toolsError } = useTools();

  // Zustand stores — select only needed fields to avoid re-renders on unrelated state changes
  const agents = useAgentStore((s) => s.agents);
  const addProjectOpen = useNavigationStore((s) => s.addProjectOpen);
  const setAddProjectOpen = useNavigationStore((s) => s.setAddProjectOpen);
  const settingsOpen = useNavigationStore((s) => s.settingsOpen);
  const setSettingsOpen = useNavigationStore((s) => s.setSettingsOpen);

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

  return (
    <ErrorBoundary>
    <ToastProvider>
    <div className="app">
      <Sidebar />
      <MainPanel />
      <CommandPalette />
      <ConfigCenter />

      {(projectError || toolsError) && <div className="error-banner" style={{position:'fixed',top:0,left:'50%',transform:'translateX(-50%)',zIndex:300}}>{projectError || toolsError}</div>}

      {addProjectOpen && (
        <AddProject onAdd={async (p) => { await addProject(p); setAddProjectOpen(false); }} onClose={() => setAddProjectOpen(false)} existingProjects={projects} />
      )}

      {settingsOpen && (
        <Settings tools={tools} agents={agents} onClose={() => setSettingsOpen(false)} />
      )}
    </div>
    </ToastProvider>
    </ErrorBoundary>
  );
}

export default App;
