import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';

// Capture the listen callback so tests can emit synthetic progress events.
// listen() returns a Promise (the real API is async); we set the handler
// synchronously so it's available after the effect's await resolves.
type Handler = (e: { payload: unknown }) => void;
let progressHandler: Handler | null = null;
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn((_evt: string, cb: Handler) => {
    progressHandler = cb;
    return Promise.resolve(() => {});
  }),
}));

import { WorkflowProgressStrip } from '../WorkflowProgressStrip';

function emit(event: Record<string, unknown>) {
  act(() => {
    progressHandler!({ payload: { run_id: 'r1', event } });
  });
}

async function waitForHandler(): Promise<Handler> {
  await vi.waitFor(() => {
    if (!progressHandler) throw new Error('listen handler not registered');
  });
  return progressHandler!;
}

describe('WorkflowProgressStrip', () => {
  beforeEach(() => {
    progressHandler = null;
  });

  it('renders nothing before any node event', async () => {
    render(<WorkflowProgressStrip />);
    await waitForHandler();
    expect(screen.queryByTestId('workflow-progress-strip')).toBeNull();
  });

  it('lights up chips as node_start/node_end events arrive', async () => {
    render(<WorkflowProgressStrip />);
    await waitForHandler();
    emit({ kind: 'node_start', node: 'w1' });
    emit({ kind: 'node_end', node: 'w1', status: 'done' });
    emit({ kind: 'node_start', node: 'w2' });
    const strip = screen.getByTestId('workflow-progress-strip');
    expect(strip).toHaveTextContent('w1');
    expect(strip).toHaveTextContent('w2');
  });

  it('counts retries from node_retried events', async () => {
    render(<WorkflowProgressStrip />);
    await waitForHandler();
    emit({ kind: 'node_start', node: 'w1' });
    emit({ kind: 'node_retried', node: 'w1', attempt: 1, error: '503' });
    emit({ kind: 'node_retried', node: 'w1', attempt: 2, error: '503' });
    expect(screen.getByTestId('workflow-progress-strip')).toHaveTextContent(/⟳2/);
  });

  it('marks a failed node', async () => {
    render(<WorkflowProgressStrip />);
    await waitForHandler();
    emit({ kind: 'node_end', node: 'w1', status: 'failed', error: 'timeout' });
    const strip = screen.getByTestId('workflow-progress-strip');
    expect(strip).toHaveTextContent('w1');
    expect(strip).toHaveTextContent(/✗/);
  });

  it('hides itself once the graph settles (graph_done)', async () => {
    render(<WorkflowProgressStrip />);
    await waitForHandler();
    emit({ kind: 'node_start', node: 'w1' });
    expect(screen.getByTestId('workflow-progress-strip')).toBeInTheDocument();
    emit({ kind: 'graph_done', output: null });
    // settle → strip hands off to the tool_result pill, renders nothing
    expect(screen.queryByTestId('workflow-progress-strip')).toBeNull();
  });

  it('hides on graph_failed too', async () => {
    render(<WorkflowProgressStrip />);
    await waitForHandler();
    emit({ kind: 'node_start', node: 'w1' });
    emit({ kind: 'graph_failed', error: 'boom' });
    expect(screen.queryByTestId('workflow-progress-strip')).toBeNull();
  });
});
