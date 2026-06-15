import type { ActivityEvent } from '../types';
import { useNavigationStore } from '../stores/navigationStore';
import { useAgentStore } from '../stores/agentStore';

interface ActivityItemProps {
  event: ActivityEvent;
}

const EVENT_ICONS: Record<string, string> = {
  session_started: '▶',
  session_completed: '✓',
  session_failed: '✕',
  quality_gate: '⚡',
};

export function ActivityItem({ event }: ActivityItemProps) {
  const icon = EVENT_ICONS[event.eventType] || '○';
  const time = new Date(event.timestamp).toLocaleString();
  const selectConversation = useNavigationStore((s) => s.selectConversation);
  const getConversationForSession = useAgentStore((s) => s.getConversationForSession);

  const handleSessionClick = () => {
    // An activity event is tied to a turn (session). Jump to the conversation
    // that turn belongs to — the conversation is the navigable unit now.
    if (event.sessionId) {
      const conv = getConversationForSession(event.sessionId);
      if (conv) selectConversation(conv.id);
    }
  };

  return (
    <div className={`activity-item ${event.eventType}`}>
      <span className="activity-item-icon">{icon}</span>
      <div className="activity-item-body">
        <span className="activity-item-title">{event.title}</span>
        {event.description && <span className="activity-item-desc">{event.description}</span>}
        <div className="activity-item-meta">
          <span className="activity-item-time">{time}</span>
          {event.sessionId && (
            <button
              className="activity-item-session-link"
              onClick={handleSessionClick}
              title="跳转到对应会话"
            >
              会话 {event.sessionId.slice(0, 8)}
            </button>
          )}
        </div>
      </div>
      <span className="activity-item-agent">{event.agentType.replace(/_/g, ' ')}</span>
    </div>
  );
}
