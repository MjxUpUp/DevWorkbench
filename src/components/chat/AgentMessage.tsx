import { useState, useMemo } from 'react';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { Session, QualityReport } from '../../types';
import { TerminalView } from '../TerminalView';
import { QualityReportPanel } from '../QualityReportPanel';
import { IconEdit, IconCpu, IconStar, IconX, IconStop } from '../Icons';

interface AgentMessageProps {
  session: Session;
  running: boolean;
  qualityReport: QualityReport | null;
  elapsed?: string;
}

const ICON_SIZE = 14;

export function AgentMessage({ session, running, qualityReport, elapsed }: AgentMessageProps) {
  const [chainCollapsed, setChainCollapsed] = useState(false);
  const [terminalCollapsed, setTerminalCollapsed] = useState(false);

  const statusDot = running ? 'running' : session.status === 'completed' ? 'completed' : 'failed';
  const statusLabel = running ? '运行中' : session.status === 'completed' ? '已完成' : '失败';

  // Build decision chain steps from session data
  const chainSteps = useMemo(() => {
    const steps: { icon: React.ReactNode; label: string; detail: string; status: 'done' | 'active' | 'pending' }[] = [];
    steps.push({ icon: <IconEdit size={ICON_SIZE} />, label: '需求定义', detail: session.prompt.slice(0, 80), status: 'done' });
    steps.push({
      icon: <IconCpu size={ICON_SIZE} />,
      label: 'Agent 执行',
      detail: `${session.agentType}${session.model ? ` · ${session.model}` : ''}`,
      status: running ? 'active' : 'done',
    });
    if (!running) {
      steps.push({
        icon: session.status === 'completed'
          ? <IconStar size={ICON_SIZE} filled />
          : session.status === 'failed'
          ? <IconX size={ICON_SIZE} />
          : <IconStop size={ICON_SIZE} />,
        label: '结果',
        detail: statusLabel,
        status: 'done',
      });
    }
    return steps;
  }, [session, running, statusLabel]);

  // File changes from context snapshot. Prefer fileDiffs (real +/- from git
  // numstat) when the backend attached them; fall back to the filesChanged
  // path list without per-file stats for older sessions.
  const fileChanges = useMemo(() => {
    const snap = session.contextSnapshot;
    if (!snap) return [];
    if (snap.fileDiffs && snap.fileDiffs.length > 0) return snap.fileDiffs;
    return (snap.filesChanged ?? []).map((path) => ({ path, added: 0, removed: 0 }));
  }, [session.contextSnapshot]);

  return (
    <div className="agent-message">
      <div className="agent-message-header">
        <span className={`agent-status-dot ${statusDot}`} />
        <span className="agent-name">{session.agentType}</span>
        {session.model && <span className="agent-model">{session.model}</span>}
        <span style={{ margin: '0 4px', color: 'var(--text-tertiary)' }}>·</span>
        <span>{statusLabel}</span>
        {elapsed && <span className="agent-elapsed">{elapsed}</span>}
      </div>

      {/* Decision Chain block */}
      {chainSteps.length > 0 && (
        <div className="agent-block">
          <div className="agent-block-header" onClick={() => setChainCollapsed(!chainCollapsed)}>
            <span className="agent-block-title">Decision Chain</span>
            <span className="agent-block-collapse">{chainCollapsed ? '▸' : '▾'}</span>
          </div>
          {!chainCollapsed && (
            <div className="agent-block-body">
              <div className="decision-chain-steps">
                {chainSteps.map((step, i) => (
                  <div key={i} className={`decision-chain-step ${step.status}`}>
                    <span className="decision-chain-step-icon">{step.icon}</span>
                    <div className="decision-chain-step-content">
                      <span className="decision-chain-step-label">{step.label}</span>
                      <span className="decision-chain-step-detail">{step.detail}</span>
                    </div>
                  </div>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      {/* Agent text output — the agent's reply, rendered as Markdown.
          outputSummary is null while running, so this block only appears for
          completed sessions (streaming live text is a separate backend feature). */}
      {session.outputSummary && (
        <div className="agent-block">
          <div className="agent-block-header">
            <span className="agent-block-title">输出</span>
          </div>
          <div className="agent-block-body agent-output">
            <ReactMarkdown remarkPlugins={[remarkGfm]}>
              {session.outputSummary}
            </ReactMarkdown>
          </div>
        </div>
      )}

      {/* Terminal Output block */}
      <div className="agent-block">
        <div className="agent-block-header" onClick={() => setTerminalCollapsed(!terminalCollapsed)}>
          <span className="agent-block-title">Terminal Output</span>
          <span className="agent-block-collapse">{terminalCollapsed ? '▸' : '▾'}</span>
        </div>
        {!terminalCollapsed && (
          <div className="agent-block-body" style={{ padding: 0 }}>
            <TerminalView
              sessionId={running ? session.id : null}
              completedSession={!running ? session : null}
            />
          </div>
        )}
      </div>

      {/* File Changes block */}
      {fileChanges.length > 0 && !running && (
        <div className="agent-block">
          <div className="agent-block-header">
            <span className="agent-block-title">File Changes</span>
            <span className="agent-block-badge">{fileChanges.length} files</span>
          </div>
          <div className="agent-block-body">
            <div className="file-changes-list">
              {fileChanges.map((file, i) => (
                <div key={i} className="file-change-item">
                  <span className="file-change-icon"><IconEdit size={ICON_SIZE} /></span>
                  <span className="file-change-path">{file.path}</span>
                  {(file.added > 0 || file.removed > 0) && (
                    <span className="file-change-stats">
                      {file.added > 0 && <span className="file-change-added">+{file.added}</span>}
                      {file.removed > 0 && <span className="file-change-removed">-{file.removed}</span>}
                    </span>
                  )}
                </div>
              ))}
            </div>
          </div>
        </div>
      )}

      {/* Quality Gate block */}
      {qualityReport && !running && (
        <div className="agent-block">
          <div className="agent-block-header">
            <span className="agent-block-title">Quality Gate</span>
            <span className="agent-block-badge">
              {qualityReport.checks.filter((c) => c.status === 'passed').length}/{qualityReport.checks.length}
              {' '}{qualityReport.overallStatus === 'passed'
                ? <IconStar size={12} filled />
                : <IconX size={12} />}
            </span>
          </div>
          <div className="agent-block-body" style={{ padding: 0 }}>
            <QualityReportPanel report={qualityReport} />
          </div>
        </div>
      )}
    </div>
  );
}
