import { useState, useMemo, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import ReactMarkdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { Session, QualityReport, ChatStreamEvent } from '../../types';
import { useAgentStore } from '../../stores/agentStore';
import { TerminalView } from '../TerminalView';
import { QualityReportPanel } from '../QualityReportPanel';
import { BlocksView } from './BlocksView';
import { IconEdit, IconCpu, IconStar, IconX, IconStop } from '../Icons';

interface AgentMessageProps {
  session: Session;
  running: boolean;
  qualityReport: QualityReport | null;
  elapsed?: string;
}

const ICON_SIZE = 14;
// Stable empty array so the blocks selector returns the SAME reference when a
// session has no blocks yet — otherwise `?? []` mints a fresh array every render
// and Zustand treats it as a state change → infinite re-render loop.
const EMPTY_BLOCKS: ChatStreamEvent[] = [];

export function AgentMessage({ session, running, qualityReport, elapsed }: AgentMessageProps) {
  const [chainCollapsed, setChainCollapsed] = useState(false);
  const [terminalCollapsed, setTerminalCollapsed] = useState(false);
  // Full agent reply for completed sessions. session.outputSummary is the
  // tail-truncated (2000-char) preview of the same log file the terminal reads,
  // so rendering it as Markdown produced a duplicated, cut-off block next to
  // the terminal. For completed sessions we instead load the FULL output via
  // read_session_output_cmd (ANSI-stripped, untruncated) and render that.
  const [fullOutput, setFullOutput] = useState<string | null>(null);

  const statusDot = running ? 'running' : session.status === 'completed' ? 'completed' : 'failed';
  const statusLabel = running ? '运行中' : session.status === 'completed' ? '已完成' : '失败';

  // Structured agent output blocks (from `agent:event`). When present, the
  // output area renders BlocksView instead of the terminal/markdown path —
  // this is the chat-blocks UI for claude (and later ReactAgent). Raw agents
  // (pi) emit no agent:event, so they keep the existing terminal/markdown path.
  // Declared before the fullOutput effect below, which short-circuits on it.
  const liveBlocks = useAgentStore((s) => s.sessionBlocks.get(session.id) ?? EMPTY_BLOCKS);
  // Live in-memory blocks win while a session is running; once finalized the
  // live Map is cleared (agent:completed) and we fall back to the persisted
  // session.blocks read from the DB — so a reloaded or switched-away session
  // still renders its block cards instead of the raw terminal log.
  const blocks = liveBlocks.length > 0 ? liveBlocks : (session.blocks ?? EMPTY_BLOCKS);
  const useBlocks = blocks.length > 0;

  // Completed session → load the full reply text once. Falls back to the
  // (truncated) outputSummary if the log file is gone, so something always shows.
  useEffect(() => {
    // BlocksView handles display when structured blocks are available — either
    // the live in-memory Map (running session) or the persisted session.blocks
    // (finalized/reloaded). Skip the full-output log load in both cases.
    if (running || useBlocks) {
      setFullOutput(null);
      return;
    }
    let cancelled = false;
    invoke<string | null>('read_session_output_cmd', { sessionId: session.id })
      .then((full) => {
        if (cancelled) return;
        setFullOutput(full ?? session.outputSummary);
      })
      .catch(() => {
        if (cancelled) return;
        setFullOutput(session.outputSummary);
      });
    return () => { cancelled = true; };
  }, [session.id, session.outputSummary, running, useBlocks]);

  // pty 缓存是否含本会话输出。区分"刚完成的当前会话"（缓存仍在，可作占位）
  // 与"历史会话重新加载"（缓存空，用 loading 占位，不显示空 terminal）。
  const hasCachedPty = useAgentStore((s) => {
    const chunks = s.ptyOutput.get(session.id);
    return !!(chunks && chunks.length > 0);
  });

  // 输出区三态互斥（避免完成瞬间 terminal 卸载残留造成的"一闪"）：
  //   running                             → Terminal 实时流式
  //   刚完成 + pty 缓存仍在 + 输出未就绪    → Terminal 占位（不卸载，直到 Markdown 同帧替换）
  //   完整输出就绪                         → Markdown 渲染
  //   历史会话 + 输出未就绪                → loading 占位
  const showTerminal = running || (!running && !fullOutput && hasCachedPty);
  const showMarkdown = !running && !!fullOutput;
  const showOutputLoading = !running && !fullOutput && !hasCachedPty;

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

      {/* 输出区（三态互斥）：
          - running：Terminal 实时流式
          - 刚完成 + pty 缓存仍在 + 完整输出未就绪：保留 Terminal 作占位。关键在于
            这里 NOT 卸载 TerminalView —— 完成→就绪之间它继续渲染 pty 缓存最后画面，
            等 fullOutput 一就绪，卸载 terminal 与挂载 markdown 在同一 React commit，
            xterm canvas 无残留空间，杜绝"一闪而过的 terminal"。
          - 完整输出就绪：Markdown 渲染（完整、未截断）
          - 历史会话（无 pty 缓存）未就绪：loading 占位 */}
      {useBlocks ? (
        <div className="agent-block">
          <div className="agent-block-header" onClick={() => setTerminalCollapsed(!terminalCollapsed)}>
            <span className="agent-block-title">输出</span>
            <span className="agent-block-collapse">{terminalCollapsed ? '▸' : '▾'}</span>
          </div>
          {!terminalCollapsed && (
            <div className="agent-block-body agent-output">
              <BlocksView events={blocks} running={running} />
            </div>
          )}
        </div>
      ) : showTerminal ? (
        <div className="agent-block">
          <div className="agent-block-header" onClick={() => setTerminalCollapsed(!terminalCollapsed)}>
            <span className="agent-block-title">{running ? 'Terminal Output' : '输出'}</span>
            <span className="agent-block-collapse">{terminalCollapsed ? '▸' : '▾'}</span>
          </div>
          {!terminalCollapsed && (
            <div className="agent-block-body" style={{ padding: 0 }}>
              <TerminalView sessionId={session.id} completedSession={null} />
            </div>
          )}
        </div>
      ) : (showMarkdown || showOutputLoading) ? (
        <div className="agent-block">
          <div className="agent-block-header">
            <span className="agent-block-title">输出</span>
            {showOutputLoading && (
              <span className="agent-block-badge" style={{ color: 'var(--text-tertiary)' }}>加载中…</span>
            )}
          </div>
          <div className="agent-block-body agent-output">
            {showMarkdown ? (
              <ReactMarkdown remarkPlugins={[remarkGfm]}>
                {fullOutput}
              </ReactMarkdown>
            ) : (
              <span style={{ color: 'var(--text-tertiary)' }}>（加载中…）</span>
            )}
          </div>
        </div>
      ) : null}

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
