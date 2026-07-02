import { useState } from 'react';
import { Modal } from '../ui/Modal/Modal';
import { Button } from '../ui/Button/Button';
import { useAgentStore } from '../../stores/agentStore';
import styles from './ApprovalModal.module.css';

/** Human Gate modal (Clutch #3). When the kernel pauses a DESTRUCTIVE tool
 *  call for interactive approval, the agentStore short-circuits the
 *  `approval_required` block into `pendingApproval` and this modal opens. The
 *  three choices map 1:1 to `resolve_human_gate_cmd`:
 *  - Approve → the tool runs as-is.
 *  - Reject  → tool result becomes "[blocked: 用户拒绝]" so the agent adapts.
 *  - Retry   → the typed feedback becomes the tool result, steering the agent.
 *
 *  Esc / overlay click = Reject (the safe default for a destructive op; never
 *  wedges the run — Esc must resolve the suspended call, not dismiss it silently
 *  or the agent hangs until the 300s timeout). Scoped to the ReactKernel chat
 *  path only; OpaqueAgent (CLI subprocess) runs without synchronous approval
 *  (Clutch's default skip-permissions parity) and never opens this modal. */
export function ApprovalModal() {
  const pending = useAgentStore((s) => s.pendingApproval);
  const resolveApproval = useAgentStore((s) => s.resolveApproval);

  // Retry feedback is the only editable input; revealed when the user picks
  // Retry so they can type the steering note before confirming.
  const [feedback, setFeedback] = useState('');
  const [mode, setMode] = useState<'choose' | 'retry'>('choose');

  // Reset local state when a new approval arrives (or none). The modal is
  // keyed off `pendingApproval`; clearing it unmounts the choice view.
  const token = pending?.resumeToken ?? null;
  if (!pending || !token) return null;

  // Pretty-print the arguments JSON for the preview — raw tool args are a JSON
  // string; a compacted key/value view reads far better than a single line.
  let prettyArgs = pending.arguments;
  try {
    prettyArgs = JSON.stringify(JSON.parse(pending.arguments), null, 2);
  } catch {
    // Not JSON (e.g. a raw shell command for bash) — show verbatim.
  }

  const handleClose = () => {
    // Esc / overlay = Reject (safe default for destructive ops).
    void resolveApproval('reject');
    setMode('choose');
    setFeedback('');
  };

  const handleConfirm = () => {
    if (mode === 'retry') {
      void resolveApproval('retry', feedback.trim() || undefined);
    } else {
      void resolveApproval('approve');
    }
    setMode('choose');
    setFeedback('');
  };

  return (
    <Modal
      open
      onClose={handleClose}
      variant="danger"
      aria-label="破坏性操作审批"
      width={560}
    >
      <Modal.Header>
        <h2 className={styles.title}>{pending.summary}</h2>
      </Modal.Header>
      <Modal.Body>
        <p className={styles.lead}>
          当前 Agent 处于「人工审批」模式，以下破坏性操作需要你确认后才会执行。
        </p>
        <dl className={styles.meta}>
          <div className={styles.metaRow}>
            <dt>工具</dt>
            <dd><code className={styles.tool}>{pending.tool}</code></dd>
          </div>
        </dl>
        <pre className={styles.args} aria-label="操作参数预览">{prettyArgs}</pre>
        {mode === 'retry' && (
          <textarea
            className={styles.feedback}
            value={feedback}
            onChange={(e) => setFeedback(e.target.value)}
            placeholder="补充给 Agent 的指令（例如：换个目录 / 跳过该步骤 / 改用更安全的方式）"
            rows={3}
            aria-label="重试反馈"
            autoFocus
          />
        )}
        <div className={styles.actions}>
          {mode === 'choose' ? (
            <>
              <Button
                variant="danger"
                onClick={() => { void resolveApproval('reject'); setMode('choose'); setFeedback(''); }}
              >
                拒绝（阻止执行）
              </Button>
              <Button variant="ghost" onClick={() => setMode('retry')}>
                重试（补充指令）
              </Button>
              <Button variant="secondary" onClick={handleConfirm} className={styles.approveBtn}>
                同意执行
              </Button>
            </>
          ) : (
            <>
              <Button variant="ghost" onClick={() => { setMode('choose'); setFeedback(''); }}>
                返回
              </Button>
              <Button variant="primary" onClick={handleConfirm} disabled={!feedback.trim()}>
                发送重试指令
              </Button>
            </>
          )}
        </div>
      </Modal.Body>
    </Modal>
  );
}
