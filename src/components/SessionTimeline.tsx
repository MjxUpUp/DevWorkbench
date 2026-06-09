import { useState } from 'react';
import type { Session, AgentType, AgentInfo } from '../types';
import { SESSION_STATUS_LABELS, SESSION_STATUS_CLASSES } from '../utils/sessionStatus';

interface SessionTimelineProps {
  sessions: Session[];
  agents: AgentInfo[];
  onContinueWith: (session: Session, targetAgent: AgentType) => void;
}

function formatTime(iso: string): string {
  return new Date(iso).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
}

function formatRelativeDate(iso: string): 'today' | 'yesterday' | 'earlier' {
  const date = new Date(iso);
  const now = new Date();
  const todayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const yesterdayStart = new Date(todayStart.getTime() - 86400000);

  if (date >= todayStart) return 'today';
  if (date >= yesterdayStart) return 'yesterday';
  return 'earlier';
}

export function SessionTimeline({ sessions, agents, onContinueWith }: SessionTimelineProps) {
  const [expandedId, setExpandedId] = useState<string | null>(null);
  const [continueFor, setContinueFor] = useState<string | null>(null);

  const sorted = [...sessions].sort((a, b) =>
    new Date(b.startedAt).getTime() - new Date(a.startedAt).getTime()
  );

  const groups: { label: string; sessions: Session[] }[] = [];
  const today = sorted.filter(s => formatRelativeDate(s.startedAt) === 'today');
  const yesterday = sorted.filter(s => formatRelativeDate(s.startedAt) === 'yesterday');
  const earlier = sorted.filter(s => formatRelativeDate(s.startedAt) === 'earlier');

  if (today.length) groups.push({ label: 'Today', sessions: today });
  if (yesterday.length) groups.push({ label: 'Yesterday', sessions: yesterday });
  if (earlier.length) groups.push({ label: 'Earlier', sessions: earlier });

  // Derive agent label from discovery data
  const getLabel = (agentType: AgentType): string => {
    const found = agents.find(a => a.agentType === agentType);
    return found?.displayName || agentType;
  };

  // Only show installed agents in the continue picker
  const installedAgents = agents.filter(a => a.installed);

  if (sessions.length === 0) {
    return (
      <div className="session-timeline">
        <div className="session-timeline-empty">暂无会话记录</div>
      </div>
    );
  }

  return (
    <div className="session-timeline">
      {groups.map(group => (
        <div key={group.label} className="session-timeline-group">
          <div className="session-timeline-group-label">{group.label}</div>
          {group.sessions.map(session => {
            const statusLabel = SESSION_STATUS_LABELS[session.status] || session.status;
            const statusClass = SESSION_STATUS_CLASSES[session.status] || 'session-badge-completed';
            const isExpanded = expandedId === session.id;
            const isContinueOpen = continueFor === session.id;

            return (
              <div key={session.id} className="session-timeline-item">
                <div
                  className="session-timeline-item-header"
                  onClick={() => setExpandedId(isExpanded ? null : session.id)}
                >
                  <span className="session-timeline-agent">
                    {getLabel(session.agentType)}
                  </span>
                  <span className="session-timeline-time">{formatTime(session.startedAt)}</span>
                  <span className={`session-timeline-badge ${statusClass}`}>{statusLabel}</span>
                </div>
                <div className="session-timeline-prompt">
                  {session.prompt.length > 60
                    ? session.prompt.slice(0, 60) + '...'
                    : session.prompt}
                </div>

                {isExpanded && (
                  <div className="session-timeline-expanded">
                    <div className="session-timeline-output">
                      {session.outputSummary ?? '(无输出摘要)'}
                    </div>
                    {session.status !== 'running' && (
                      <div className="session-timeline-continue">
                        {isContinueOpen ? (
                          <div className="session-continue-agents">
                            {installedAgents.map(agent => (
                              <button
                                key={agent.agentType}
                                className="session-continue-agent-btn"
                                onClick={() => {
                                  onContinueWith(session, agent.agentType);
                                  setContinueFor(null);
                                }}
                              >
                                {agent.displayName}
                              </button>
                            ))}
                          </div>
                        ) : (
                          <button
                            className="session-continue-btn"
                            onClick={() => setContinueFor(session.id)}
                          >
                            Continue with another agent
                          </button>
                        )}
                      </div>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>
      ))}
    </div>
  );
}
