import { beforeEach, describe, expect, it, vi } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invoke(...args),
}));

import { useKnowledgeStore } from '../knowledgeStore';
import type { KnowledgeEntry } from '../../types';

function deferred<T>() {
  let resolve!: (v: T) => void;
  const promise = new Promise<T>((r) => {
    resolve = r;
  });
  return { promise, resolve };
}

function entry(id: string): KnowledgeEntry {
  return { id } as unknown as KnowledgeEntry;
}

describe('useKnowledgeStore — search/load race guards', () => {
  beforeEach(() => {
    invoke.mockReset();
    useKnowledgeStore.setState({ entries: [], searchResults: [], loading: false });
  });

  it('search: drops stale results when a newer query supersedes an older one', async () => {
    const slowOld = deferred<KnowledgeEntry[]>();
    const fastNew = deferred<KnowledgeEntry[]>();
    invoke.mockReturnValueOnce(slowOld.promise).mockReturnValueOnce(fastNew.promise);

    const pOld = useKnowledgeStore.getState().search('first');
    const pNew = useKnowledgeStore.getState().search('second'); // supersedes

    fastNew.resolve([entry('n1')]);
    await pNew;
    expect(useKnowledgeStore.getState().searchResults.map((e) => e.id)).toEqual(['n1']);

    slowOld.resolve([entry('o1')]);
    await pOld;
    // old result dropped; the stale `finally` must not toggle loading off the
    // active request either.
    expect(useKnowledgeStore.getState().searchResults.map((e) => e.id)).toEqual(['n1']);
    expect(useKnowledgeStore.getState().loading).toBe(false);
  });

  it('loadForProject: drops stale entries on rapid project switch', async () => {
    const slowP1 = deferred<KnowledgeEntry[]>();
    const fastP2 = deferred<KnowledgeEntry[]>();
    invoke.mockReturnValueOnce(slowP1.promise).mockReturnValueOnce(fastP2.promise);

    const p1 = useKnowledgeStore.getState().loadForProject('proj1');
    const p2 = useKnowledgeStore.getState().loadForProject('proj2'); // supersedes

    fastP2.resolve([entry('p2')]);
    await p2;
    expect(useKnowledgeStore.getState().entries.map((e) => e.id)).toEqual(['p2']);

    slowP1.resolve([entry('p1')]);
    await p1;
    expect(useKnowledgeStore.getState().entries.map((e) => e.id)).toEqual(['p2']);
    expect(useKnowledgeStore.getState().loading).toBe(false);
  });

  it('search and loadForProject use independent sequences and do not clobber each other', async () => {
    const s1 = deferred<KnowledgeEntry[]>();
    const l1 = deferred<KnowledgeEntry[]>();
    invoke.mockReturnValueOnce(s1.promise).mockReturnValueOnce(l1.promise);

    const ps = useKnowledgeStore.getState().search('q');
    const pl = useKnowledgeStore.getState().loadForProject('p');

    // Resolve in the opposite order than they started; both data fields must be
    // independently preserved (entries vs searchResults are separate slices).
    l1.resolve([entry('loaded')]);
    await pl;
    s1.resolve([entry('found')]);
    await ps;

    expect(useKnowledgeStore.getState().entries.map((e) => e.id)).toEqual(['loaded']);
    expect(useKnowledgeStore.getState().searchResults.map((e) => e.id)).toEqual(['found']);
  });
});
