import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { evalApi } from '../evalApi';

describe('evalApi', () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it('runSession invokes eval_run_session with camelCase params', async () => {
    vi.mocked(invoke).mockResolvedValue({
      id: 'r1',
      session_id: 's1',
      conversation_id: null,
      matcher: 'exact_match',
      score: 1.0,
      grade: 'optimal',
      steps: 3,
      created_at: '2026-06-20T00:00:00Z',
    });
    const r = await evalApi.runSession('s1', 'in_order', ['read', 'grep']);
    expect(invoke).toHaveBeenCalledWith('eval_run_session', {
      sessionId: 's1',
      matcher: 'in_order',
      reference: ['read', 'grep'],
    });
    expect(r.grade).toBe('optimal');
    expect(r.score).toBe(1.0);
  });

  it('runSession defaults matcher to exact_match and omits reference', async () => {
    vi.mocked(invoke).mockResolvedValue({
      id: 'r2',
      session_id: 's1',
      conversation_id: null,
      matcher: 'exact_match',
      score: 0.0,
      grade: 'incorrect',
      steps: 0,
      created_at: '2026-06-20T00:00:00Z',
    });
    await evalApi.runSession('s1');
    expect(invoke).toHaveBeenCalledWith('eval_run_session', {
      sessionId: 's1',
      matcher: 'exact_match',
      reference: undefined,
    });
  });

  it('listRuns forwards optional scope + limit', async () => {
    vi.mocked(invoke).mockResolvedValue([]);
    await evalApi.listRuns('s1', 10);
    expect(invoke).toHaveBeenCalledWith('list_eval_runs', {
      sessionId: 's1',
      limit: 10,
    });
  });

  it('trend returns daily buckets', async () => {
    vi.mocked(invoke).mockResolvedValue([
      { date: '2026-06-19', avg_score: 0.8, count: 3 },
      { date: '2026-06-20', avg_score: 1.0, count: 1 },
    ]);
    const pts = await evalApi.trend(7);
    expect(invoke).toHaveBeenCalledWith('eval_trend', { days: 7 });
    expect(pts).toHaveLength(2);
    expect(pts[0].avg_score).toBe(0.8);
  });
});
