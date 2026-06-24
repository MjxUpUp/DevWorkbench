import { useEffect, type ReactNode, type HTMLAttributes } from 'react';
import { Button } from '../Button/Button';
import { IconX } from '../../Icons';
import styles from './Modal.module.css';

/**
 * Modal primitive — compound 组件，统一替代散落的 .modal-overlay/.modal/
 * .modal-header/.modal-close/.modal-body className（原 AddProject 自实现）。
 *
 * 内建 a11y（WAI-ARIA dialog pattern）：
 * - role="dialog" aria-modal="true"
 * - Esc 关闭（统一，消费方不再自写 keydown）
 * - overlay 点击关闭 / content 点击不冒泡
 *
 * 视觉规格消费 token（variables.css），含 dialog 表单控件样式
 * （从 .modal-body input/label 迁移，保证迁移零视觉 diff）。
 */

interface RootProps {
  /** 是否显示。false 时返回 null（不渲染 DOM）。 */
  open: boolean;
  /** 关闭回调（Esc / overlay 点击触发）。 */
  onClose: () => void;
  children: ReactNode;
  /** 对话框无障碍标签（屏幕阅读器朗读）。 */
  'aria-label'?: string;
  /** 内容宽度，默认 520px（对齐原 .modal）。 */
  width?: number | string;
  /** 视觉变体。danger 用于危险确认（删除/不可撤销操作），角标与边框用陶土色。 */
  variant?: 'default' | 'danger';
}

export function Modal({ open, onClose, children, ...rest }: RootProps) {
  // Esc 关闭 — 消费方不再需要自写 keydown 监听
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open, onClose]);

  if (!open) return null;

  const { 'aria-label': ariaLabel, width, variant = 'default' } = rest;

  return (
    <div className={styles.overlay} onClick={onClose}>
      <div
        className={styles.content}
        role="dialog"
        aria-modal="true"
        aria-label={ariaLabel}
        data-modal-variant={variant}
        onClick={(e) => e.stopPropagation()}
        style={width !== undefined ? { width } : undefined}
      >
        {/* pi.dev 四角取景框（下两角用 span，上两角用伪元素）*/}
        <span className={styles['corner-bl']} aria-hidden="true" />
        <span className={styles['corner-br']} aria-hidden="true" />
        {children}
      </div>
    </div>
  );
}

type HeaderProps = HTMLAttributes<HTMLDivElement>;
function Header({ className, ...props }: HeaderProps) {
  return <div className={`${styles.header} ${className ?? ''}`} {...props} />;
}

type BodyProps = HTMLAttributes<HTMLDivElement>;
function Body({ className, ...props }: BodyProps) {
  return <div className={`${styles.body} ${className ?? ''}`} {...props} />;
}

interface CloseProps {
  onClose: () => void;
  'aria-label'?: string;
}
function Close({ onClose, 'aria-label': ariaLabel = '关闭' }: CloseProps) {
  return (
    <Button
      variant="ghost"
      size="sm"
      onClick={onClose}
      aria-label={ariaLabel}
      leadingIcon={<IconX size={16} />}
    />
  );
}

Modal.Header = Header;
Modal.Body = Body;
Modal.Close = Close;
