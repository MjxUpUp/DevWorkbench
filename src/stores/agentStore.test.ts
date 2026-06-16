import { describe, it, expect, vi, beforeEach } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { invoke } from '@tauri-apps/api/core';
import { useAgentStore } from './agentStore';
import type { Session, Conversation } from '../types';

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
  conversationId: null,
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

describe('agentStore — conversation selectors', () => {
  beforeEach(() => {
    useAgentStore.setState({ sessions: [], conversations: [] });
  });

  const conv = (id: string, over: Partial<Conversation> = {}): Conversation => ({
    id,
    projectPath: '/p',
    title: `t-${id}`,
    lastAgent: 'claude_code',
    status: 'active',
    startedAt: '2026-01-01T00:00:00Z',
    lastActivityAt: '2026-01-01T00:00:00Z',
    pinned: false,
    ...over,
  });

  it('getTurnsForConversation returns only that conversation\'s turns, oldest-first', () => {
    useAgentStore.setState({
      sessions: [
        mk({ id: 'b', conversationId: 'c1', startedAt: '2026-01-02T00:00:00Z' }),
        mk({ id: 'x', conversationId: 'other' }),
        mk({ id: 'a', conversationId: 'c1', startedAt: '2026-01-01T00:00:00Z' }),
        mk({ id: 'c', conversationId: 'c1', startedAt: '2026-01-03T00:00:00Z' }),
      ],
    });

    const turns = useAgentStore.getState().getTurnsForConversation('c1');
    expect(turns.map((t) => t.id)).toEqual(['a', 'b', 'c']);
  });

  it('getConversationsForProject sorts pinned first then by last activity desc', () => {
    useAgentStore.setState({
      conversations: [
        conv('old', { lastActivityAt: '2026-01-01T00:00:00Z' }),
        conv('new', { lastActivityAt: '2026-02-01T00:00:00Z' }),
        conv('pinned', { lastActivityAt: '2026-01-15T00:00:00Z', pinned: true }),
        conv('other-proj', { projectPath: '/q' }),
      ],
    });

    const list = useAgentStore.getState().getConversationsForProject('/p');
    // pinned floats above newer activity; then newest-first.
    expect(list.map((c) => c.id)).toEqual(['pinned', 'new', 'old']);
  });

  it('getConversationForSession resolves the container of a turn', () => {
    useAgentStore.setState({
      sessions: [mk({ id: 'turn1', conversationId: 'c1' })],
      conversations: [conv('c1')],
    });
    expect(useAgentStore.getState().getConversationForSession('turn1')?.id).toBe('c1');
    // A turn with no conversation_id (pre-migration orphan) resolves to null.
    useAgentStore.setState({ sessions: [mk({ id: 'orphan', conversationId: null })] });
    expect(useAgentStore.getState().getConversationForSession('orphan')).toBeNull();
  });
});

describe('agentStore.appendBlock — merge consecutive text deltas', () => {
  beforeEach(() => {
    useAgentStore.setState({ sessionBlocks: new Map() });
  });

  it('concatenates content when the last block is also text (real streaming)', () => {
    // Real streaming emits one Text event per token; appendBlock must fold them
    // into a single block so BlocksView doesn't render one card per delta.
    useAgentStore.getState().appendBlock('s1', { kind: 'text', content: 'hel' });
    useAgentStore.getState().appendBlock('s1', { kind: 'text', content: 'lo' });
    useAgentStore.getState().appendBlock('s1', { kind: 'text', content: ' world' });
    const blocks = useAgentStore.getState().sessionBlocks.get('s1');
    expect(blocks).toEqual([{ kind: 'text', content: 'hello world' }]);
  });

  it('does not merge when the previous block is a different kind', () => {
    useAgentStore.getState().appendBlock('s1', { kind: 'tool_use', name: 'Read', input: { file_path: '/x' } });
    useAgentStore.getState().appendBlock('s1', { kind: 'text', content: 'done' });
    const blocks = useAgentStore.getState().sessionBlocks.get('s1');
    expect(blocks).toEqual([
      { kind: 'tool_use', name: 'Read', input: { file_path: '/x' } },
      { kind: 'text', content: 'done' },
    ]);
  });

  it('resumes merging after a non-text block splits the run', () => {
    useAgentStore.getState().appendBlock('s1', { kind: 'text', content: 'a' });
    useAgentStore.getState().appendBlock('s1', { kind: 'tool_use', name: 'Read', input: {} });
    useAgentStore.getState().appendBlock('s1', { kind: 'text', content: 'b' });
    useAgentStore.getState().appendBlock('s1', { kind: 'text', content: 'c' });
    const blocks = useAgentStore.getState().sessionBlocks.get('s1');
    expect(blocks).toEqual([
      { kind: 'text', content: 'a' },
      { kind: 'tool_use', name: 'Read', input: {} },
      { kind: 'text', content: 'bc' },
    ]);
  });

  it('keeps sessions isolated', () => {
    useAgentStore.getState().appendBlock('a', { kind: 'text', content: '1' });
    useAgentStore.getState().appendBlock('b', { kind: 'text', content: '2' });
    useAgentStore.getState().appendBlock('a', { kind: 'text', content: '3' });
    expect(useAgentStore.getState().sessionBlocks.get('a')).toEqual([{ kind: 'text', content: '13' }]);
    expect(useAgentStore.getState().sessionBlocks.get('b')).toEqual([{ kind: 'text', content: '2' }]);
  });

  it('merges consecutive thinking deltas the same way as text', () => {
    // GLM interleaved thinking streams as multiple Thinking deltas; they must
    // fold into one block so a long reasoning trace renders as one card, not
    // one card per token.
    useAgentStore.getState().appendBlock('s1', { kind: 'thinking', content: 'first ' });
    useAgentStore.getState().appendBlock('s1', { kind: 'thinking', content: 'then ' });
    useAgentStore.getState().appendBlock('s1', { kind: 'thinking', content: 'last' });
    const blocks = useAgentStore.getState().sessionBlocks.get('s1');
    expect(blocks).toEqual([{ kind: 'thinking', content: 'first then last' }]);
  });

  it('does not merge thinking into a preceding text block (different kinds stay separate)', () => {
    useAgentStore.getState().appendBlock('s1', { kind: 'text', content: 'answer' });
    useAgentStore.getState().appendBlock('s1', { kind: 'thinking', content: 'reasoning' });
    useAgentStore.getState().appendBlock('s1', { kind: 'text', content: ' more' });
    const blocks = useAgentStore.getState().sessionBlocks.get('s1');
    expect(blocks).toEqual([
      { kind: 'text', content: 'answer' },
      { kind: 'thinking', content: 'reasoning' },
      { kind: 'text', content: ' more' },
    ]);
  });
});

describe('agentStore.spawnAgent — permission mode threading (v1.1 Task 5)', () => {
  beforeEach(() => {
    useAgentStore.setState({ sessions: [], conversations: [] });
    vi.clearAllMocks();
    // spawnAgent awaits invoke (Session) then refreshConversations awaits invoke
    // (Conversation[]) — one mock return value satisfies both.
    vi.mocked(invoke).mockResolvedValue(mk());
  });

  it('passes the selected mode through to the spawn_agent_session payload', async () => {
    await useAgentStore.getState().spawnAgent(
      '/p', 'claude_code', 'hi', undefined, undefined, undefined, undefined, false, 'plan',
    );
    const [cmd, payload] = vi.mocked(invoke).mock.calls[0];
    expect(cmd).toBe('spawn_agent_session');
    expect(payload).toMatchObject({ mode: 'plan' });
  });

  it('defaults mode to null when none selected (backend maps null → Default)', async () => {
    await useAgentStore.getState().spawnAgent('/p', 'claude_code', 'hi');
    const [, payload] = vi.mocked(invoke).mock.calls[0];
    expect(payload).toMatchObject({ mode: null });
  });

  it('createConversation forwards mode to spawnAgent', async () => {
    await useAgentStore.getState().createConversation('/p', 'hi', 'claude_code', false, 'skip-permissions');
    const [, payload] = vi.mocked(invoke).mock.calls[0];
    expect(payload).toMatchObject({ mode: 'skip-permissions' });
  });

  it('continueConversation forwards mode to spawnAgent', async () => {
    await useAgentStore.getState().continueConversation('/p', 'c1', 'hi', 'claude_code', false, 'auto-edit');
    const [, payload] = vi.mocked(invoke).mock.calls[0];
    expect(payload).toMatchObject({ mode: 'auto-edit' });
  });
});
