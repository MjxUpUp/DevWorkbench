import { useAgentStore } from '../../stores/agentStore';
import { useNavigationStore } from '../../stores/navigationStore';

export function AgentStatusBar() {
  const sessions = useAgentStore((s) => s.sessions);
  const activeProject = useNavigationStore((s) => s.activeProject);

  const runningSessions = sessions.filter(s => s.status === 'running');
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
              {activeProject && s.projectPath === activeProject.path && ' · active'}
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
