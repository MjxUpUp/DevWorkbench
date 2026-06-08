import type { Requirement, Session } from '../types';

interface RequirementCardProps {
  requirement: Requirement;
  sessions: Session[];
  onStart: (id: string) => void;
  onMarkDone: (id: string) => void;
}

const STATUS_CONFIG: Record<string, { label: string; className: string }> = {
  todo: { label: 'Todo', className: 'req-status-todo' },
  in_progress: { label: 'In Progress', className: 'req-status-in-progress' },
  done: { label: 'Done', className: 'req-status-done' },
};

export function RequirementCard({ requirement, sessions, onStart, onMarkDone }: RequirementCardProps) {
  const config = STATUS_CONFIG[requirement.status] ?? STATUS_CONFIG.todo;

  const linkedSession = requirement.linkedSessionId
    ? sessions.find(s => s.id === requirement.linkedSessionId)
    : null;

  return (
    <div className={`requirement-card ${config.className}`}>
      <div className="requirement-card-header">
        <span className={`requirement-status-badge ${config.className}`}>
          {config.label}
        </span>
        {linkedSession && (
          <span className="requirement-session-link">
            {linkedSession.agentType} · {linkedSession.status === 'running' ? '运行中' : linkedSession.status === 'completed' ? '完成' : '失败'}
          </span>
        )}
      </div>
      <div className="requirement-card-title">{requirement.title}</div>
      {requirement.description && (
        <div className="requirement-card-desc">{requirement.description}</div>
      )}
      <div className="requirement-card-actions">
        {requirement.status === 'todo' && (
          <button className="requirement-action-btn start" onClick={() => onStart(requirement.id)}>
            Start
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
