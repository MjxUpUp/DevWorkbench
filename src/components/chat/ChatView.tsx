import { useMemo } from 'react';
import { useNavigationStore } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';
import { DecisionChain } from './DecisionChain';
import { FileChanges } from './FileChanges';
import { AgentPanel } from '../AgentPanel';

/**
 * Chat view — wraps AgentPanel and adds structured output blocks:
 * 1. Decision Chain (collapsible)
 * 2. Terminal Output (inside AgentPanel)
 * 3. File Changes
 * 4. Quality Gate (inside AgentPanel via QualityReportPanel)
 */
export function ChatView() {
  const project = useNavigationStore((s) => s.activeProject);
  const activeSessionId = useNavigationStore((s) => s.selectedSessionId);
  const allSessions = useAgentStore((s) => s.sessions);
  const getSessionsForProject = useAgentStore((s) => s.getSessionsForProject);

  const projectSessions = useMemo(
    () => project ? getSessionsForProject(project.path) : [],
    [getSessionsForProject, project?.path, allSessions]
  );

  const runningSession = useMemo(
    () => projectSessions.find(s => s.status === 'running') ?? null,
    [projectSessions]
  );

  const displaySession = runningSession ?? (
    activeSessionId ? allSessions.find(s => s.id === activeSessionId) ?? null : null
  );

  const allRequirements = useAgentStore((s) => s.requirements);
  const activeRequirement = useMemo(() => {
    if (activeSessionId && project) {
      return allRequirements.find(r =>
        r.projectPath === project.path && r.linkedSessionId === activeSessionId
      ) ?? null;
    }
    return null;
  }, [allRequirements, activeSessionId, project]);

  const hasConversation = !!(activeSessionId || runningSession);

  if (!project) {
    // Landing state — show AgentPanel's built-in landing
    return <AgentPanel />;
  }

  return (
    <div className="chat-view">
      <div className="chat-view-blocks">
        {/* Block 1: Decision Chain */}
        {hasConversation && displaySession && (
          <DecisionChain
            requirement={activeRequirement}
            session={displaySession}
            running={!!runningSession}
          />
        )}

        {/* Block 2 & 4: Terminal Output + Quality Gate (inside AgentPanel) */}
        {/* Block 3: File Changes */}
        {displaySession && displaySession.status !== 'running' && (
          <FileChanges session={displaySession} />
        )}
      </div>

      {/* AgentPanel handles header, body (terminal), composer */}
      <AgentPanel />
    </div>
  );
}
