import { useState, useMemo, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
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
  // Full agent reply for completed sessions. session.outputSummary is the
  // tail-truncated (2000-char) preview of the same log file the terminal reads,
  // so rendering it as Markdown produced a duplicated, cut-off block next to
  // the terminal. For completed sessions we instead load the FULL output via
  // read_session_output_cmd (ANSI-stripped, untruncated) and render that.
  const [fullOutput, setFullOutput] = useState<string | null>(null);
  const [fullOutputLoading, setFullOutputLoading] = useState(false);

  const statusDot = running ? 'running' : session.status === 'completed' ? 'completed' : 'failed';
  const statusLabel = running ? '运行中' : session.status === 'completed' ? '已完成' : '失败';

  // Completed session → load the full reply text once. Falls back to the
  // (truncated) outputSummary if the log file is gone, so something always shows.
  useEffect(() => {
    if (running) {
      setFullOutput(null);
      return;
    }
    let cancelled = false;
    setFullOutputLoading(true);
    invoke<string | null>('read_session_output_cmd', { sessionId: session.id })
      .then((full) => {
        if (cancelled) return;
        setFullOutput(full ?? session.outputSummary);
      })
      .catch(() => {
        if (cancelled) return;
        setFullOutput(session.outputSummary);
      })
      .finally(() => {
        if (!cancelled) setFullOutputLoading(false);
      });
    return () => { cancelled = true; };
  }, [session.id, session.outputSummary, running]);

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

      {/* Completed session: the agent's full reply, rendered as Markdown.
          Reads the COMPLETE output via read_session_output_cmd (the same log
          the terminal used), not the tail-truncated outputSummary — so it's
          neither cut off nor duplicated by the terminal block below. */}
      {!running && (fullOutput || fullOutputLoading) && (
        <div className="agent-block">
          <div className="agent-block-header">
            <span className="agent-block-title">输出</span>
            {fullOutputLoading && (
              <span className="agent-block-badge" style={{ color: 'var(--text-tertiary)' }}>加载中…</span>
            )}
          </div>
          <div className="agent-block-body agent-output">
            {fullOutput ? (
              <ReactMarkdown remarkPlugins={[remarkGfm]}>
                {fullOutput}
              </ReactMarkdown>
            ) : (
              <span style={{ color: 'var(--text-tertiary)' }}>（无输出记录）</span>
            )}
          </div>
        </div>
      )}

      {/* Running session: live terminal stream. For completed sessions the full
          reply is already rendered as Markdown above, so we don't show the raw
          terminal again — that was the duplicated "two replies" symptom. */}
      {running && (
        <div className="agent-block">
          <div className="agent-block-header" onClick={() => setTerminalCollapsed(!terminalCollapsed)}>
            <span className="agent-block-title">Terminal Output</span>
            <span className="agent-block-collapse">{terminalCollapsed ? '▸' : '▾'}</span>
          </div>
          {!terminalCollapsed && (
            <div className="agent-block-body" style={{ padding: 0 }}>
              <TerminalView sessionId={session.id} completedSession={null} />
            </div>
          )}
        </div>
      )}

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
