import { useState, type ReactNode, type HTMLAttributes } from 'react';
import styles from './L2ToolPill.module.css';

/**
 * L2ToolPill — 工具调用折叠 pill。
 *
 * Cursor 3.0 / Codex app 三段折叠范式的 L2 层（中抽象）：
 * - 单行 pill「✓ tool_name · 描述 · 耗时」，compact 摘要
 * - 展开看完整输入/输出（diff、参数、结果）
 * - 默认折叠（dig deeper when you want）
 *
 * 对应 ChatStreamEvent.kind = 'tool_use' + 关联的 'tool_result'。
 *
 * status 控制图标与边框：
 * - success：✓ 绿（已完成）
 * - running：▸ 橙（进行中）
 * - error：✕ 红（失败，pill 红边框）
 *
 * a11y：button 控制展开，aria-expanded 同步。
 * 透传 ...props 到根 div。
 */
export type ToolStatus = 'success' | 'running' | 'error';

export interface L2ToolPillProps {
  /** 工具名（read_file / edit_file / dispatch_subagent / bash ...）。 */
  name: string;
  /** 单行描述（文件名 + 行数 / 命令 / 子任务摘要）。 */
  desc: string;
  /** 工具状态。success=✓绿 / running=▸橙 / error=✕红。 */
  status?: ToolStatus;
  /** 元信息（耗时 / 进度 / checkpoint）。 */
  meta?: string;
  /** 展开后显示的详情（diff、输入参数、输出结果）。 */
  children?: ReactNode;
  /** 默认展开态。默认 false。 */
  defaultExpanded?: boolean;
  className?: string;
  /** name span 的 data-testid（供 E2E 定位工具名，如 chat-block-tool-name）。
   *  L2ToolPill 是通用层，不硬编码业务 testid；由父组件（BlocksView 的
   *  ToolUsePill）透传，让 capstone E2E 能锁定 tool_use 的工具名单元格。 */
  nameTestId?: string;
  /** 展开 button 的 data-testid（供 E2E 定位 head，如 chat-block-toolresult-head）。 */
  headTestId?: string;
}

const ICON: Record<ToolStatus, string> = {
  success: '✓',
  running: '▸',
  error: '✕',
};

export function L2ToolPill({
  name,
  desc,
  status = 'success',
  meta,
  children,
  defaultExpanded = false,
  className,
  nameTestId,
  headTestId,
  ...props
}: L2ToolPillProps & HTMLAttributes<HTMLDivElement>) {
  const [expanded, setExpanded] = useState(defaultExpanded);
  const wrapClasses = [
    styles.wrap,
    expanded ? styles.expanded : '',
    status === 'error' ? styles.errWrap : '',
    className,
  ].filter(Boolean).join(' ');
  const pillClasses = [styles.pill, status === 'error' ? styles.err : '']
    .filter(Boolean)
    .join(' ');

  return (
    <div className={wrapClasses} {...props}>
      <button
        type="button"
        data-testid={headTestId}
        className={pillClasses}
        onClick={() => setExpanded((v) => !v)}
        aria-expanded={expanded}
        aria-label={`${name} 工具调用${expanded ? '（折叠详情）' : '（展开详情）'}`}
      >
        <span className={`${styles.icon} ${styles[status]}`} aria-hidden="true">{ICON[status]}</span>
        <span className={styles.name} data-testid={nameTestId}>{name}</span>
        <span className={styles.desc}>{desc}</span>
        {meta && <span className={styles.meta}>{meta}</span>}
        {children && <span className={styles.chev} aria-hidden="true">›</span>}
      </button>
      {expanded && children && (
        <div className={styles.detail} role="region" aria-label={`${name} 调用详情`}>
          {children}
        </div>
      )}
    </div>
  );
}
