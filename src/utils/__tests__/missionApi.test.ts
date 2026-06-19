import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));

import { invoke } from '@tauri-apps/api/core';
import { missionApi } from '../missionApi';

describe('missionApi', () => {
  beforeEach(() => vi.mocked(invoke).mockReset());

  it('init invokes mission_init with the mission id (camelCase param)', async () => {
    vi.mocked(invoke).mockResolvedValue({
      currentPhase: 'plan',
      iteration: 0,
      maxIterations: 20,
    });
    const r = await missionApi.init('m1');
    expect(invoke).toHaveBeenCalledWith('mission_init', { missionId: 'm1' });
    expect(r.currentPhase).toBe('plan');
  });

  it('loadPrd returns valid + problems + corrupted shape', async () => {
    vi.mocked(invoke).mockResolvedValue({
      valid: false,
      problems: ["Missing top-level 'userStories' array"],
      prd: null,
      corrupted: false,
    });
    const r = await missionApi.loadPrd('m1');
    expect(invoke).toHaveBeenCalledWith('mission_load_prd', { missionId: 'm1' });
    expect(r.valid).toBe(false);
    expect(r.problems).toHaveLength(1);
    expect(r.corrupted).toBe(false);
  });

  it('apply flips to executing phase', async () => {
    vi.mocked(invoke).mockResolvedValue({
      currentPhase: 'executing',
      iteration: 0,
      maxIterations: 20,
    });
    const r = await missionApi.apply('mission-abc');
    expect(invoke).toHaveBeenCalledWith('mission_apply', {
      missionId: 'mission-abc',
    });
    expect(r.currentPhase).toBe('executing');
  });

  it('status returns phase + live pass count', async () => {
    vi.mocked(invoke).mockResolvedValue({
      state: { currentPhase: 'executing', iteration: 3, maxIterations: 20 },
      passed: 2,
      total: 5,
      corrupted: false,
    });
    const r = await missionApi.status('m1');
    expect(invoke).toHaveBeenCalledWith('mission_status', { missionId: 'm1' });
    expect(r.state.iteration).toBe(3);
    expect(`${r.passed}/${r.total}`).toBe('2/5');
  });
});
