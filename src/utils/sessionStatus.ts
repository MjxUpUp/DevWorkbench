import type { SessionStatus, RequirementStatus } from '../types';

/**
 * Session status → Chinese display label.
 * Single canonical mapping — all components should use this.
 */
export const SESSION_STATUS_LABELS: Record<SessionStatus, string> = {
  running: '运行中',
  completed: '完成',
  failed: '失败',
};

/**
 * Session status → badge CSS class name.
 */
export const SESSION_STATUS_CLASSES: Record<SessionStatus, string> = {
  running: 'session-badge-running',
  completed: 'session-badge-completed',
  failed: 'session-badge-failed',
};

/**
 * Requirement status → display label.
 */
export const REQUIREMENT_STATUS_LABELS: Record<RequirementStatus, string> = {
  todo: '待办',
  in_progress: '进行中',
  done: '已完成',
};

/**
 * Requirement status → badge CSS class name.
 */
export const REQUIREMENT_STATUS_CLASSES: Record<RequirementStatus, string> = {
  todo: 'req-status-todo',
  in_progress: 'req-status-in-progress',
  done: 'req-status-done',
};
