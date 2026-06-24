import styles from './Stripe.module.css';

/**
 * Stripe — pi.dev 双色硬切激活条。
 *
 * pi.dev 状态激活签名：light 主题单色 accent，dark 主题 accent 62%→thread-blue
 * 硬切（62% 处一刀切，非渐变）。
 *
 * 用于：状态栏激活段、BudgetBar 填充、分隔强调、卡片底部强调条。
 *
 * a11y：纯装饰，aria-hidden。
 */
export interface StripeProps {
  /** 条高。sm=3px（细线）/ md=6px（标准）/ lg=8px（强调）。 */
  height?: 'sm' | 'md' | 'lg';
  /** 宽度。full=100%（默认）/ auto=随内容。 */
  width?: 'full' | 'auto';
  className?: string;
}

export function Stripe({ height = 'md', width = 'full', className }: StripeProps) {
  const classes = [
    styles.stripe,
    styles[`h-${height}`],
    styles[`w-${width}`],
    className,
  ]
    .filter(Boolean)
    .join(' ');
  return <span className={classes} aria-hidden="true" />;
}
