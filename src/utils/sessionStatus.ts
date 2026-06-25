import type { SessionStatus } from '../types';

/**
 * Session status → Chinese display label.
 * Single canonical mapping — all components should use this.
 */
export const SESSION_STATUS_LABELS: Record<SessionStatus, string> = {
  running: '运行中',
  completed: '完成',
  failed: '失败',
  cancelled: '已取消',
};

/**
 * Session status → badge CSS class name.
 */
export const SESSION_STATUS_CLASSES: Record<SessionStatus, string> = {
  running: 'session-badge-running',
  completed: 'session-badge-completed',
  failed: 'session-badge-failed',
  cancelled: 'session-badge-cancelled',
};
