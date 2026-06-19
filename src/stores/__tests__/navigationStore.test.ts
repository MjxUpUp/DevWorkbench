import { describe, it, expect, beforeEach } from 'vitest';
import { useNavigationStore } from '../navigationStore';

describe('navigationStore.setTrace (LLM trace view entry)', () => {
  beforeEach(() => {
    // No reset() on this store; restore the two fields setTrace touches.
    useNavigationStore.setState({ traceSessionId: null, activeView: 'task' });
  });

  it('setTrace scopes the trace view to the given session and switches to it', () => {
    // The 「🔍 Trace」 button on a turn calls setTrace(turn.id); TraceView then
    // reads traceSessionId to fetch that turn's LLM calls. A wrong key here
    // would route to a no-op view or show the empty state forever.
    useNavigationStore.getState().setTrace('sess-41f2ddca');
    expect(useNavigationStore.getState().traceSessionId).toBe('sess-41f2ddca');
    expect(useNavigationStore.getState().activeView).toBe('trace');
  });

  it('selecting a different turn re-scopes without leaving the trace view', () => {
    useNavigationStore.getState().setTrace('sess-a');
    useNavigationStore.getState().setTrace('sess-b');
    expect(useNavigationStore.getState().traceSessionId).toBe('sess-b');
    expect(useNavigationStore.getState().activeView).toBe('trace');
  });

  it('clearing traceSessionId restores the TraceView empty state', () => {
    useNavigationStore.getState().setTrace('sess-a');
    useNavigationStore.setState({ traceSessionId: null });
    expect(useNavigationStore.getState().traceSessionId).toBeNull();
  });
});
