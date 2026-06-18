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
  // "复制ID" 反馈：点击后短暂显示"已复制"。session.id 是排查后端日志/DB 的
  // 唯一键——用户报问题时复制 id 给排查方，比复述原始 prompt 精确省时得多
  // （否则只能靠 prompt 文本盲查会话）。
  const [idCopied, setIdCopied] = useState(false);

  const copySessionId = async () => {
    try {
      await navigator.clipboard.writeText(session.id);
      setIdCopied(true);
      window.setTimeout(() => setIdCopied(false), 1500);
    } catch {
      // clipboard 不可用时退化为选中 prompt（webview 非 secure / 权限拒绝等）
    }
  };
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
  // Structured agents (claude_code / react_kernel / gemini_cli / qwen_code) emit
  // `agent:event` blocks and must NEVER render the terminal form — even before
  // the first block arrives (e.g. the model gateway is holding its response).
  // Without this gate, claude showed a terminal box "等待输出" during that
  // running-but-empty window, contradicting the B-plan goal of eliminating the
  // terminal form for structured agents. gemini_cli/qwen_code run in
  // `-o stream-json` (same structured reader path as claude), so they join here.
  // Raw agents (pi/codex/…) emit only pty:output bytes, so they keep the
  // terminal path. `showBlocks` drives the render branch below; BlocksView
  // itself renders a chat-blocks "waiting" hint when empty + running.
  const isStructured =
    session.agentType === 'claude_code' ||
    session.agentType === 'react_kernel' ||
    session.agentType === 'gemini_cli' ||
    session.agentType === 'qwen_code';
  // Structured agents reach the BlocksView form in EVERY state — running with
  // zero blocks (gateway holding the response → waiting hint), running with
  // accumulating blocks, AND completed (persisted session.blocks). The previous
  // `(isStructured && running)` gate left a hole: on `agent:completed` the live
  // sessionBlocks Map is cleared and refreshSessions' DB read is async, so for a
  // frame `useBlocks=false` + `running=false` → showBlocks=false → the render
  // fell through to the terminal/loading branch → a terminal box flashed before
  // the persisted blocks loaded (the "终端闪现" symptom). Dropping `&& running`
  // closes it: structured agents NEVER render the terminal form, matching the
  // design intent stated in the comment above. An empty BlocksView (completed
  // turn whose blocks are still loading) is harmless and self-corrects on the
  // next render once session.blocks arrives.
  const showBlocks = useBlocks || isStructured;

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
        <button
          type="button"
          className={`agent-message-copy-id${idCopied ? ' copied' : ''}`}
          title={`会话 ID（点击复制，便于报障排查后端日志/DB）：${session.id}`}
          onClick={copySessionId}
        >
          {idCopied ? '已复制' : '复制ID'}
        </button>
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

      {/* 输出区（结构化 agent 永远走 chat-blocks，含等待态；raw agent 三态）：
          - 结构化（claude/react_kernel）有 blocks / 运行中：BlocksView（运行且无
            block 时 BlocksView 渲染"等待模型响应"等待态，绝不回退 terminal）
          - raw agent running：Terminal 实时流式
          - raw 刚完成 + pty 缓存仍在 + 完整输出未就绪：保留 Terminal 作占位（NOT
            卸载 TerminalView，完成→就绪同帧 swap，杜绝 terminal"一闪"）
          - raw 完整输出就绪：Markdown 渲染（完整、未截断）
          - raw 历史会话（无 pty 缓存）未就绪：loading 占位 */}
      {showBlocks ? (
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
