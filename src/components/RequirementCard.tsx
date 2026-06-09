import type { Requirement, Session, AgentInfo } from '../types';
import { REQUIREMENT_STATUS_LABELS, REQUIREMENT_STATUS_CLASSES, SESSION_STATUS_LABELS } from '../utils/sessionStatus';

interface RequirementCardProps {
  requirement: Requirement;
  sessions: Session[];
  agents?: AgentInfo[];
  onStart: (id: string) => void;
  onMarkDone: (id: string) => void;
  onContinue: (id: string) => void;
}

export function RequirementCard({ requirement, sessions, agents = [], onStart, onMarkDone, onContinue }: RequirementCardProps) {
  const statusLabel = REQUIREMENT_STATUS_LABELS[requirement.status] || requirement.status;
  const statusClass = REQUIREMENT_STATUS_CLASSES[requirement.status] || 'req-status-todo';

  const linkedSession = requirement.linkedSessionId
    ? sessions.find(s => s.id === requirement.linkedSessionId)
    : null;

  const getAgentLabel = (agentType: string): string => {
    const found = agents.find(a => a.agentType === agentType);
    return found?.displayName || agentType;
  };

  return (
    <div className={`requirement-card ${statusClass}`}>
      <div className="requirement-card-header">
        <span className={`requirement-status-badge ${statusClass}`}>
          {statusLabel}
        </span>
        {linkedSession && (
          <span className="requirement-session-link">
            {getAgentLabel(linkedSession.agentType)} · {SESSION_STATUS_LABELS[linkedSession.status] || linkedSession.status}
          </span>
        )}
      </div>
      <div className="requirement-card-title">{requirement.title}</div>
      {requirement.description && (
        <div className="requirement-card-desc">{requirement.description}</div>
      )}
      <div className="requirement-card-actions">
        {requirement.status === 'todo' && !requirement.linkedSessionId && (
          <button className="requirement-action-btn start" onClick={() => onStart(requirement.id)}>
            Start
          </button>
        )}
        {requirement.status === 'todo' && requirement.linkedSessionId && (
          <button className="requirement-action-btn continue" onClick={() => onContinue(requirement.id)}>
            Continue
          </button>
        )}
        {requirement.status === 'in_progress' && (
          <button className="requirement-action-btn done" onClick={() => onMarkDone(requirement.id)}>
            Mark Done
          </button>
        )}
      </div>
    </div>
  );
}
