import { forwardRef, type InputHTMLAttributes, type TextareaHTMLAttributes, type LabelHTMLAttributes, type ReactNode } from 'react';
import styles from './Input.module.css';

/**
 * Input primitive — 统一表单输入，替代裸 <input>/<textarea> + 手写 <label>。
 *
 * 价值（相对 base.css 的全局 input reset）：
 * - a11y 内建：invalid → aria-invalid + danger border（base.css 无此能力）
 * - 视觉统一：消费 token，对齐 Modal.Body 的 input 规格（8px 12px / radius-sm）
 * - label 关联：Label + Input 组合，屏幕阅读器正确朗读
 *
 * 视觉规格消费 token（variables.css），禁止 hex 字面量。
 */

interface InputProps extends InputHTMLAttributes<HTMLInputElement> {
  /** 校验失败态：aria-invalid + danger border。 */
  invalid?: boolean;
}

export const Input = forwardRef<HTMLInputElement, InputProps>(function Input(
  { invalid, className, ...props },
  ref,
) {
  return (
    <input
      ref={ref}
      className={`${styles.input}${invalid ? ` ${styles.invalid}` : ''}${className ? ` ${className}` : ''}`}
      aria-invalid={invalid || undefined}
      {...props}
    />
  );
});

interface TextareaProps extends TextareaHTMLAttributes<HTMLTextAreaElement> {
  invalid?: boolean;
}

export const Textarea = forwardRef<HTMLTextAreaElement, TextareaProps>(function Textarea(
  { invalid, className, ...props },
  ref,
) {
  return (
    <textarea
      ref={ref}
      className={`${styles.input}${invalid ? ` ${styles.invalid}` : ''}${className ? ` ${className}` : ''}`}
      aria-invalid={invalid || undefined}
      {...props}
    />
  );
});

interface LabelProps extends LabelHTMLAttributes<HTMLLabelElement> {
  children: ReactNode;
}

export const Label = forwardRef<HTMLLabelElement, LabelProps>(function Label(
  { className, children, ...props },
  ref,
) {
  return (
    <label ref={ref} className={`${styles.label}${className ? ` ${className}` : ''}`} {...props}>
      {children}
    </label>
  );
});
