import type { Session, AgentInfo } from '../types';
import { SESSION_STATUS_LABELS } from '../utils/sessionStatus';
import { formatRelativeTime } from '../utils/formatRelativeTime';

interface AgentStatusProps {
  sessions: Session[];
  agents?: AgentInfo[];
}

export function AgentStatus({ sessions, agents = [] }: AgentStatusProps) {
  if (sessions.length === 0) return null;

  const getAgentLabel = (agentType: string): string => {
    const found = agents.find(a => a.agentType === agentType);
    return found?.displayName || agentType;
  };

  const runningSession = sessions.find(s => s.status === 'running');

  if (runningSession) {
    return (
      <div className="agent-status agent-status-running">
        <span className="agent-status-dot agent-status-dot-running" />
        <span className="agent-status-text">
          {getAgentLabel(runningSession.agentType)} 运行中
        </span>
      </div>
    );
  }

  const latest = sessions.sort((a, b) =>
    new Date(b.startedAt).getTime() - new Date(a.startedAt).getTime()
  )[0];

  const statusLabel = SESSION_STATUS_LABELS[latest.status] || latest.status;
  const statusClass = latest.status === 'completed' ? 'completed' : 'failed';

  return (
    <div className={`agent-status agent-status-${statusClass}`}>
      <span className={`agent-status-dot agent-status-dot-${statusClass}`} />
      <span className="agent-status-text">
        {getAgentLabel(latest.agentType)} · {formatRelativeTime(latest.startedAt)} · {statusLabel}
      </span>
    </div>
  );
}
