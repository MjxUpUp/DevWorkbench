import { useState, useMemo } from 'react';
import type { Session, QualityReport, ChatStreamEvent } from '../../types';
import { useAgentStore } from '../../stores/agentStore';
import { useNavigationStore } from '../../stores/navigationStore';
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

  // 「🔍 Trace」: jump to the LLM trace view scoped to THIS turn. The only side
  // effect is the navigation-store swap; TraceView fetches the rows itself.
  const setTrace = useNavigationStore((s) => s.setTrace);

  const copySessionId = async () => {
    try {
      await navigator.clipboard.writeText(session.id);
      setIdCopied(true);
      window.setTimeout(() => setIdCopied(false), 1500);
    } catch {
      // clipboard 不可用时退化为选中 prompt（webview 非 secure / 权限拒绝等）
    }
  };

  const statusDot = running ? 'running' : session.status === 'completed' ? 'completed' : 'failed';
  const statusLabel = running ? '运行中' : session.status === 'completed' ? '已完成' : '失败';

  // Structured agent output blocks. ReactKernel is the sole agent now, so every
  // session renders BlocksView: live in-memory blocks win while running; once
  // finalized the live Map is cleared (agent:completed) and we fall back to the
  // persisted session.blocks read from the DB. Historical raw-agent sessions
  // (legacy pi/codex with no agent:event stream) have no blocks and render
  // BlocksView's empty state — the terminal raw-output path is gone.
  const liveBlocks = useAgentStore((s) => s.sessionBlocks.get(session.id) ?? EMPTY_BLOCKS);
  const blocks = liveBlocks.length > 0 ? liveBlocks : (session.blocks ?? EMPTY_BLOCKS);

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
    <div className="agent-message" data-testid="agent-message">
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
        <button
          type="button"
          className="agent-message-copy-id"
          title="查看本 turn 的每一次 LLM HTTP 调用（请求体 / 响应体 / 状态 / 延迟）"
          onClick={() => setTrace(session.id)}
        >
          🔍 Trace
        </button>
      </div>

      {/* Decision Chain block */}
      {chainSteps.length > 0 && (
        <div className="agent-block">
          <button type="button" className="agent-block-header" aria-expanded={!chainCollapsed} onClick={() => setChainCollapsed(!chainCollapsed)}>
            <span className="agent-block-title">Decision Chain</span>
            <span className="agent-block-collapse">{chainCollapsed ? '▸' : '▾'}</span>
          </button>
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

      {/* 输出区：ReactKernel 唯一 agent，恒走 chat-blocks（BlocksView）。运行中且
          无 block 时 BlocksView 渲染"等待模型响应"等待态；完成后从持久化
          session.blocks 回放。raw terminal/Markdown 路径已随 CLI 退役删除。 */}
      <div className="agent-block">
        <button type="button" className="agent-block-header" aria-expanded={!terminalCollapsed} onClick={() => setTerminalCollapsed(!terminalCollapsed)}>
          <span className="agent-block-title">输出</span>
          <span className="agent-block-collapse">{terminalCollapsed ? '▸' : '▾'}</span>
        </button>
        {!terminalCollapsed && (
          <div className="agent-block-body agent-output">
            <BlocksView events={blocks} running={running} sessionId={session.id} />
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
