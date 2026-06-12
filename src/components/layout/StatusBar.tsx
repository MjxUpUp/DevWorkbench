import { useNavigationStore } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';

export function StatusBar() {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const sessions = useAgentStore((s) => s.sessions);

  const runningCount = sessions.filter(s => s.status === 'running').length;
  const projectName = activeProject?.name ?? 'No project';

  return (
    <footer className="status-bar">
      <div className="status-bar-left">
        <span className="status-bar-item" title={activeProject?.path}>
          ◉ {projectName}
        </span>
        {runningCount > 0 && (
          <span className="status-bar-item running">
            ● {runningCount} running
          </span>
        )}
      </div>
      <div className="status-bar-right">
        <span className="status-bar-item">Forge ✓</span>
        <span className="status-bar-item">v1.0</span>
      </div>
    </footer>
  );
}
