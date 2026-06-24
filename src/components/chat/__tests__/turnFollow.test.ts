import { describe, it, expect } from 'vitest';
import { shouldFollowLatest } from '../turnFollow';

const t = (id: string, parent?: string | null) => ({ id, parentSessionId: parent ?? null });

describe('shouldFollowLatest', () => {
  it('returns false when there are no turns', () => {
    expect(shouldFollowLatest([], null)).toBe(false);
    expect(shouldFollowLatest([], 't1')).toBe(false);
  });

  it('follows the newest turn when the leaf is unset (fresh / just mounted view)', () => {
    expect(shouldFollowLatest([t('t1', null)], null)).toBe(true);
  });

  it('follows the newest turn when the leaf is stale (vanished from turns)', () => {
    expect(shouldFollowLatest([t('t1', null), t('t2', 't1')], 'gone')).toBe(true);
  });

  it('REGRESSION: follows a turn that is a direct child of the current leaf (continuation)', () => {
    // History t1 → t2, user viewing t2 (the leaf). New turn t3 is a child of
    // t2 — a natural continuation. Before the fix, activeLeafId stayed on t2
    // and visibleTurns' upward walk never reached t3, so the user's follow-up
    // message + the agent's streaming reply did not render until a remount.
    const turns = [t('t1', null), t('t2', 't1'), t('t3', 't2')];
    expect(shouldFollowLatest(turns, 't2')).toBe(true);
  });

  it('does NOT follow a sibling fork whose parent is not the leaf (A4 manual switch)', () => {
    // Editing t2 and regenerating forks t2fork under t1 (NOT under leaf t3).
    // The branch switcher handles viewing it — no auto-follow.
    const turns = [t('t1', null), t('t2', 't1'), t('t3', 't2'), t('t2fork', 't1')];
    expect(shouldFollowLatest(turns, 't3')).toBe(false);
  });

  it('does not re-follow once the leaf already IS the newest (stable, no loop)', () => {
    const turns = [t('t1', null), t('t2', 't1')];
    expect(shouldFollowLatest(turns, 't2')).toBe(false);
  });
});
