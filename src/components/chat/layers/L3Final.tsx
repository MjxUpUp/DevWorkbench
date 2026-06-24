import { type ReactNode } from 'react';
import styles from './L3Final.module.css';

/**
 * L3Final — 最终结论卡。
 *
 * Cursor 3.0 / Codex app 三段折叠范式的 L3 层（一等公民）：
 * - 默认展开（用户首要看的）
 * - highlight Frame 四角取景框强调（pi.dev 签名）
 * - DONE badge + 衬线斜体标题 + 正文 + 操作区
 *
 * 对应 ChatStreamEvent.kind = 'text'（agent 最终文字）+ 'result'（状态）。
 *
 * status 控制整体态：
 * - done（默认）：accent 色 badge "DONE"
 * - running：success 色 badge "RUNNING" + CRT 闪烁点
 * - error：error 色 badge + 红色四角
 *
 * a11y：article 语义 + aria-label。
 */
export type FinalStatus = 'done' | 'running' | 'error';

export interface L3FinalProps {
  /** 结论标题（衬线斜体显示）。 */
  title: string;
  /** 状态。done=accent / running=success+闪 / error=红。 */
  status?: FinalStatus;
  /** badge 文字。默认按 status 自动（DONE/RUNNING/FAILED）。 */
  badge?: string;
  /** 正文（支持 Markdown 渲染后的 HTML）。 */
  children: ReactNode;
  /** 操作区（Apply diff / Branch / Reject 等按钮）。 */
  actions?: ReactNode;
  className?: string;
}

const DEFAULT_BADGE: Record<FinalStatus, string> = {
  done: 'DONE',
  running: 'RUNNING',
  error: 'FAILED',
};

export function L3Final({
  title,
  status = 'done',
  badge,
  children,
  actions,
  className,
}: L3FinalProps) {
  const classes = [
    styles.wrap,
    status === 'error' ? styles.error : '',
    status === 'running' ? styles.running : '',
    className,
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <article className={classes} aria-label="最终结论">
      <span className={styles.bl} aria-hidden="true" />
      <span className={styles.br} aria-hidden="true" />
      <header className={styles.head}>
        <span className={styles.badge}>{badge ?? DEFAULT_BADGE[status]}</span>
        <h4 className={styles.title}>{title}</h4>
      </header>
      <div className={styles.body}>{children}</div>
      {actions && <div className={styles.actions}>{actions}</div>}
    </article>
  );
}
