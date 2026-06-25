import { describe, it, expect } from 'vitest';
import { SESSION_STATUS_LABELS, SESSION_STATUS_CLASSES } from './sessionStatus';
import type { SessionStatus } from '../types';

describe('sessionStatus', () => {
  // B1: backend emits "cancelled" (models.rs SessionStatus::Cancelled →
  // serde "cancelled" via stop_agent_session). The frontend union and BOTH
  // Records must cover it, or `LABELS["cancelled"]` is undefined → blank badge
  // / downstream `.toString()` crash every time a user stops an agent.
  it('covers the cancelled status the backend emits', () => {
    const statuses: SessionStatus[] = ['running', 'completed', 'failed', 'cancelled'];
    for (const s of statuses) {
      expect(SESSION_STATUS_LABELS[s], `${s} label missing`).toBeTruthy();
      expect(SESSION_STATUS_CLASSES[s], `${s} class missing`).toBeTruthy();
    }
    expect(SESSION_STATUS_LABELS.cancelled).toBe('已取消');
    expect(SESSION_STATUS_CLASSES.cancelled).toBe('session-badge-cancelled');
  });
});
