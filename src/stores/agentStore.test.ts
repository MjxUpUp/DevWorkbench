import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { invoke } from '@tauri-apps/api/core';
import { useAgentStore } from './agentStore';
import type { Session } from '../types';

const base: Session = {
  id: 's1',
  projectPath: '/p',
  agentType: 'claude_code',
  status: 'running',
  prompt: '',
  model: null,
  startedAt: '2026-01-01T00:00:00Z',
  finishedAt: null,
  exitCode: null,
  outputSummary: null,
  contextSnapshot: null,
  linkedRequirementId: null,
  parentSessionId: null,
};
const mk = (over: Partial<Session> = {}): Session => ({ ...base, ...over });

describe('agentStore.refreshSessions — merge (regression: running session wiped on project switch)', () => {
  beforeEach(() => {
    useAgentStore.setState({ sessions: [] });
    vi.clearAllMocks();
  });

  it('preserves an in-memory running session the DB read did not return', async () => {
    const running = mk({ id: 's-running', status: 'running' });
    useAgentStore.setState({ sessions: [running] });
    // DB returns nothing — e.g. WAL stale snapshot at the moment agent:started
    // fires refreshSessions, before the spawn write is visible to the read.
    vi.mocked(invoke).mockResolvedValue([]);

    await useAgentStore.getState().refreshSessions();

    const sessions = useAgentStore.getState().sessions;
    expect(sessions.map((s) => s.id)).toContain('s-running');
  });

  it('keeps the DB row authoritative when it includes the session', async () => {
    useAgentStore.setState({ sessions: [mk({ id: 's1', status: 'running' })] });
    vi.mocked(invoke).mockResolvedValue([mk({ id: 's1', status: 'completed' })]);

    await useAgentStore.getState().refreshSessions();

    const s = useAgentStore.getState().sessions.find((x) => x.id === 's1');
    expect(s?.status).toBe('completed');
  });

  it('merges DB rows with in-memory-only sessions without duplicates', async () => {
    useAgentStore.setState({ sessions: [mk({ id: 'mem', status: 'running' })] });
    vi.mocked(invoke).mockResolvedValue([mk({ id: 'db', status: 'completed' })]);

    await useAgentStore.getState().refreshSessions();

    const ids = useAgentStore.getState().sessions.map((s) => s.id).sort();
    expect(ids).toEqual(['db', 'mem']);
  });
});
