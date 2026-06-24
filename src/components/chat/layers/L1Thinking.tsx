import { useState, type ReactNode, type HTMLAttributes } from 'react';
import styles from './L1Thinking.module.css';

/**
 * L1Thinking — thinking 内容折叠卡。
 *
 * Cursor 3.0 / Codex app 三段折叠范式的 L1 层（最高抽象）：
 * - 默认折叠为「THOUGHT FOR Ns — 摘要」单行（dig deeper when you want）
 * - 展开看完整推理流
 * - 运行中时 label 带 CRT 闪烁点
 *
 * 对应 ChatStreamEvent.kind = 'thinking'。
 *
 * a11y：button 控制展开，aria-expanded 同步；body 用 region 语义。
 * 透传 ...props 到根 div（data-testid 等 native 属性）。
 */
export interface L1ThinkingProps {
  /** thinking 耗时（秒），显示在 label "THOUGHT FOR Ns"。 */
  secs?: number;
  /** 折叠态显示的摘要文案（一行，溢出省略）。 */
  summary: string;
  /** 展开后显示的完整推理流（原始 thinking 内容）。 */
  children: ReactNode;
  /** 是否运行中（true 时 label 带 CRT 闪烁点）。 */
  running?: boolean;
  /** token 计数（可选，显示在右侧）。 */
  tokens?: number;
  /** 默认展开态。默认 false（折叠）。 */
  defaultExpanded?: boolean;
  className?: string;
}

export function L1Thinking({
  secs,
  summary,
  children,
  running = false,
  tokens,
  defaultExpanded = false,
  className,
  ...props
}: L1ThinkingProps & HTMLAttributes<HTMLDivElement>) {
  const [expanded, setExpanded] = useState(defaultExpanded);
  const classes = [
    styles.wrap,
    expanded ? styles.expanded : '',
    running ? styles.running : '',
    className,
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <div className={classes} {...props}>
      <button
        type="button"
        className={styles.head}
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
        aria-label={expanded ? '折叠思考过程' : '展开思考过程'}
      >
        <span className={styles.chev} aria-hidden="true">›</span>
        <span className={styles.label}>
          {secs !== undefined ? `THOUGHT FOR ${secs}s` : 'THINKING'}
        </span>
        <span className={styles.summary}>— {summary}</span>
        {tokens !== undefined && (
          <span className={styles.tok} title={`${tokens} tokens`}>{tokens} tok</span>
        )}
      </button>
      {expanded && (
        <div className={styles.body} role="region" aria-label="思考过程详情">
          {children}
        </div>
      )}
    </div>
  );
}
