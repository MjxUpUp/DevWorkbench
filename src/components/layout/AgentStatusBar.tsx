import { useAgentStore } from '../../stores/agentStore';
import { useNavigationStore } from '../../stores/navigationStore';

export function AgentStatusBar() {
  const sessions = useAgentStore((s) => s.sessions);
  const activeProject = useNavigationStore((s) => s.activeProject);

  // Scope to the active project so running sessions from other projects don't
  // leak into the bar after switching projects (acceptance: cross-project leak).
  const projectSessions = activeProject
    ? sessions.filter(s => s.projectPath === activeProject.path)
    : sessions;
  const runningSessions = projectSessions.filter(s => s.status === 'running');
  const runningCount = runningSessions.length;

  return (
    <div className="agent-status-bar">
      {runningCount > 0 && (
        <>
          <span className="agent-status-indicator running" />
          <span className="agent-status-text">
            {runningCount} running
          </span>
          {runningSessions.slice(0, 3).map(s => (
            <span key={s.id} className="agent-status-chip">
              {s.agentType}
            </span>
          ))}
        </>
      )}
      {runningCount === 0 && (
        <span className="agent-status-text idle">No agents running</span>
      )}
    </div>
  );
}
