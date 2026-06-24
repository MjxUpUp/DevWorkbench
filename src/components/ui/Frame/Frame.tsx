import { forwardRef, type HTMLAttributes, type ReactNode } from 'react';
import styles from './Frame.module.css';

/**
 * Frame — pi.dev 四角取景框容器。
 *
 * pi.dev 最强个性签名：用 4 个伪角标代替整圈 border，像相机取景框。
 * 所有面板/卡片容器都套 Frame，统一视觉签名。
 *
 * variant 控制角标颜色与粗细：
 * - default：accent 色 2px（基准，多数面板）
 * - highlight：accent 色 3px 加粗（L3 最终结论、active 卡片）
 * - subtle：tertiary 色 1.5px（thinking block、tool result 次要区）
 * - success/danger/warning：语义态（tool_result 成功/失败、危险确认）
 *
 * a11y：纯装饰性角标，aria-hidden，不影响语义。
 */
export type FrameVariant =
  | 'default'
  | 'highlight'
  | 'subtle'
  | 'success'
  | 'danger'
  | 'warning';

export interface FrameProps extends HTMLAttributes<HTMLDivElement> {
  /** 取景框变体，控制角标颜色与粗细。default 为基准。 */
  variant?: FrameVariant;
  /** 是否显示四角标记。默认 true（设 false 退化为普通 bordered 容器）。 */
  corners?: boolean;
  children?: ReactNode;
}

export const Frame = forwardRef<HTMLDivElement, FrameProps>(function Frame(
  { variant = 'default', corners = true, className, children, ...props },
  ref,
) {
  const classes = [styles.frame, className].filter(Boolean).join(' ');
  return (
    <div ref={ref} className={classes} data-frame-variant={variant} {...props}>
      {corners && (
        <>
          <span className={`${styles.corner} ${styles.cornerTopLeft}`} aria-hidden="true" />
          <span className={`${styles.corner} ${styles.cornerTopRight}`} aria-hidden="true" />
          <span className={`${styles.corner} ${styles.cornerBottomLeft}`} aria-hidden="true" />
          <span className={`${styles.corner} ${styles.cornerBottomRight}`} aria-hidden="true" />
        </>
      )}
      {children}
    </div>
  );
});
