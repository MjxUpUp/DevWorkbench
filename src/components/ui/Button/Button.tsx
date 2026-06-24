import { forwardRef, type ButtonHTMLAttributes, type ReactNode } from 'react';
import styles from './Button.module.css';

/**
 * Button primitive — 统一替代散落的 20+ 碎片 className
 * (provider-btn / btn btn-primary / primary-btn / turn-edit-btn / composer-send-btn / ...)。
 *
 * 视觉规格消费 token（见 variables.css），禁止 hex 字面量。
 * 规格对齐现有 .btn（最广泛使用）：padding 4px 12px / radius 6px / weight 500。
 *
 * 修正的既有 bug：
 * - .btn-primary dark mode 用 --always-white（浅紫 accent 上对比度差）→ 统一 --text-on-accent
 * - .provider-btn padding 硬编码 6px 14px（违反 4px grid）→ 统一 token grid
 */
type Variant = 'primary' | 'secondary' | 'ghost' | 'danger' | 'dangerGhost' | 'dashed';
type Size = 'sm' | 'md';

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  /** 视觉变体。secondary 为默认（等价 .btn 基础态）。*/
  variant?: Variant;
  /** 尺寸。md 默认（4px 12px），sm 用于密集行内（4px 8px）。 */
  size?: Size;
  /** 按钮文字前的图标。 */
  leadingIcon?: ReactNode;
  /** 按钮文字后的图标。 */
  trailingIcon?: ReactNode;
  /** 加载态：禁用按钮 + aria-busy，由消费方决定是否渲染 spinner。 */
  isLoading?: boolean;
}

const variantClass: Record<Variant, string> = {
  primary: styles.primary,
  secondary: '',
  ghost: styles.ghost,
  danger: styles.danger,
  dangerGhost: styles.dangerGhost,
  dashed: styles.dashed,
};

export const Button = forwardRef<HTMLButtonElement, ButtonProps>(function Button(
  {
    variant = 'secondary',
    size = 'md',
    leadingIcon,
    trailingIcon,
    isLoading = false,
    disabled,
    className,
    children,
    type = 'button',
    ...props
  },
  ref,
) {
  const classes = [
    styles.button,
    variantClass[variant],
    size === 'sm' ? styles.sm : '',
    className ?? '',
  ]
    .filter(Boolean)
    .join(' ');

  return (
    <button
      ref={ref}
      type={type}
      className={classes}
      disabled={disabled || isLoading}
      aria-busy={isLoading || undefined}
      {...props}
    >
      {leadingIcon && (
        <span className={styles.icon} aria-hidden="true">
          {leadingIcon}
        </span>
      )}
      {children}
      {trailingIcon && (
        <span className={styles.icon} aria-hidden="true">
          {trailingIcon}
        </span>
      )}
    </button>
  );
});
