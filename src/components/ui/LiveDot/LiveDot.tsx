import styles from './LiveDot.module.css';

/**
 * LiveDot — pi.dev CRT 步进闪烁状态点。
 *
 * pi.dev 终端美学签名：用 steps(1,end) 模拟 CRT 颗粒闪烁，非平滑 opacity。
 * 用于 agent 运行状态、会话活跃指示、实时进度。
 *
 * status 控制颜色：
 * - running（默认）：成功绿 + 闪烁（agent 运行中）
 * - success：成功绿 + 闪烁
 * - warning：警告橙 + 闪烁
 * - error：错误珊瑚 + 闪烁
 * - idle：tertiary 灰 + 静态（不闪）
 *
 * a11y：纯装饰性指示，必须配合相邻的可见文字（如 "RUNNING"），
 * 不应单独靠颜色传达状态（WCAG 1.4.1）。
 */
export type LiveDotStatus = 'running' | 'success' | 'warning' | 'error' | 'idle';
export type LiveDotSize = 'sm' | 'md' | 'lg';

export interface LiveDotProps {
  status?: LiveDotStatus;
  size?: LiveDotSize;
  className?: string;
}

export function LiveDot({ status = 'running', size = 'md', className }: LiveDotProps) {
  const classes = [
    styles.dot,
    styles[size],
    styles[status],
    status === 'idle' ? styles.static : '',
    className,
  ]
    .filter(Boolean)
    .join(' ');

  return <span className={classes} aria-hidden="true" />;
}
