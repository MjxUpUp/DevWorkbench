import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));
// agentStore calls `listen` at construction-free sites only inside
// initEventListeners, which we never invoke here; stub it so the import is clean.
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn() }));

import { useAgentStore } from '../agentStore';
import type { Conversation } from '../../types';

function conv(id: string, projectPath = '/p'): Conversation {
  return {
    id,
    projectPath,
    title: `conv-${id}`,
    lastAgent: null,
    status: 'active',
    startedAt: '2026-01-01T00:00:00Z',
    lastActivityAt: '2026-01-01T00:00:00Z',
    pinned: false,
  } as unknown as Conversation;
}

describe('useAgentStore — conversation archive/delete refresh', () => {
  beforeEach(() => {
    invoke.mockReset();
    useAgentStore.setState({ conversations: [], sessions: [] });
  });

  it('deleteConversation: drops the row even though list_conversations no longer returns it (WAL-lag merge must not re-add it)', async () => {
    // The bug: refreshConversations' merge preserves local-only entries (to
    // survive WAL lag on NEW conversations). A just-deleted conversation is
    // absent from the active-only DB list — identical signal — so the merge
    // re-added it and the sidebar only cleared after an app restart.
    useAgentStore.setState({ conversations: [conv('c1'), conv('c2')] });
    invoke.mockResolvedValueOnce(undefined); // delete_conversation
    invoke.mockResolvedValueOnce([conv('c2')]); // list_conversations (c1 soft-deleted → filtered out)

    await useAgentStore.getState().deleteConversation('c1', '/p');

    expect(useAgentStore.getState().conversations.map((c) => c.id)).toEqual(['c2']);
    expect(invoke).toHaveBeenCalledWith('delete_conversation', { id: 'c1' });
  });

  it('archiveConversation: drops the row from the sidebar immediately', async () => {
    useAgentStore.setState({ conversations: [conv('c1'), conv('c2')] });
    invoke.mockResolvedValueOnce(undefined); // archive_conversation
    invoke.mockResolvedValueOnce([conv('c2')]); // list_conversations (c1 archived → filtered out)

    await useAgentStore.getState().archiveConversation('c1', '/p');

    expect(useAgentStore.getState().conversations.map((c) => c.id)).toEqual(['c2']);
    expect(invoke).toHaveBeenCalledWith('archive_conversation', { id: 'c1' });
  });

  it('refreshConversations still preserves a genuine local-only conversation (WAL-lag protection intact)', async () => {
    // Proves the fix is targeted: a conversation the DB genuinely doesn't know
    // about yet (just spawned, write not flushed) is still kept by the merge —
    // only archive/delete remove rows, via the optimistic local drop.
    useAgentStore.setState({ conversations: [conv('local-only', '/p')] });
    invoke.mockResolvedValueOnce([]); // DB sees nothing yet

    await useAgentStore.getState().refreshConversations('/p');

    expect(useAgentStore.getState().conversations.map((c) => c.id)).toEqual(['local-only']);
  });

  it('restore path re-surfaces a deleted conversation (undo works)', async () => {
    // After delete (optimistically removed), restore→active makes list_conversations
    // return it again; the merge re-adds it. This is the undo contract.
    useAgentStore.setState({ conversations: [conv('c1')] });
    // delete
    invoke.mockResolvedValueOnce(undefined);
    invoke.mockResolvedValueOnce([]);
    await useAgentStore.getState().deleteConversation('c1', '/p');
    expect(useAgentStore.getState().conversations.map((c) => c.id)).toEqual([]);
    // restore (mirrors Sidebar's undo onClick)
    invoke.mockResolvedValueOnce(undefined); // restore_conversation
    invoke.mockResolvedValueOnce([conv('c1')]); // list_conversations (active again)
    await invoke('restore_conversation', { id: 'c1' });
    await useAgentStore.getState().refreshConversations('/p');
    expect(useAgentStore.getState().conversations.map((c) => c.id)).toEqual(['c1']);
  });
});
