import type { Session, AgentInfo } from '../types';

interface AgentStatusProps {
  sessions: Session[];
  agents: AgentInfo[];
}

const AGENT_LABELS: Record<string, string> = {
  claude_code: 'Claude Code',
  codex: 'Codex',
  cursor_agent: 'Cursor Agent',
  gemini_cli: 'Gemini CLI',
  copilot: 'Copilot',
  qwen_code: 'Qwen Code',
};

function formatRelativeTime(isoTime: string): string {
  const now = Date.now();
  const then = new Date(isoTime).getTime();
  const diffSec = Math.floor((now - then) / 1000);

  if (diffSec < 60) return '刚刚';
  if (diffSec < 3600) return `${Math.floor(diffSec / 60)} 分钟前`;
  if (diffSec < 86400) return `${Math.floor(diffSec / 3600)} 小时前`;
  return `${Math.floor(diffSec / 86400)} 天前`;
}

export function AgentStatus({ sessions, agents }: AgentStatusProps) {
  if (sessions.length === 0) return null;

  const runningSession = sessions.find(s => s.status === 'running');

  if (runningSession) {
    return (
      <div className="agent-status agent-status-running">
        <span className="agent-status-dot agent-status-dot-running" />
        <span className="agent-status-text">
          {AGENT_LABELS[runningSession.agentType] ?? runningSession.agentType} 运行中
        </span>
      </div>
    );
  }

  const latest = sessions.sort((a, b) =>
    new Date(b.startedAt).getTime() - new Date(a.startedAt).getTime()
  )[0];

  const statusLabel = latest.status === 'completed' ? '完成' : '失败';
  const statusClass = latest.status === 'completed' ? 'completed' : 'failed';

  return (
    <div className={`agent-status agent-status-${statusClass}`}>
      <span className={`agent-status-dot agent-status-dot-${statusClass}`} />
      <span className="agent-status-text">
        {AGENT_LABELS[latest.agentType] ?? latest.agentType} · {formatRelativeTime(latest.startedAt)} · {statusLabel}
      </span>
    </div>
  );
}
