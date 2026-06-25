import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { OrchestrateView } from '../OrchestrateView';
import { useNavigationStore } from '../../../stores/navigationStore';
import { useOrchestrateStore } from '../../../stores/orchestrateStore';

// React Flow needs canvas measuring / ResizeObserver jsdom lacks — stub the
// canvas the same way WorkflowBuilder.test.tsx does.
vi.mock('@xyflow/react', () => {
  const ReactFlow = (props: any) => (
    <div data-testid="react-flow">
      {props.nodes?.map((n: any) => (
        <div key={n.id} data-testid={`rf-node-${n.id}`}>{n.data?.node?.id ?? n.id}</div>
      ))}
    </div>
  );
  return {
    ReactFlow,
    Background: () => null,
    BackgroundVariant: { Dots: 'dots' },
    Controls: () => null,
    Handle: () => null,
    Position: { Left: 'left', Right: 'right' },
    MiniMap: () => null,
    useReactFlow: () => ({ fitView: () => {} }),
  };
});

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(() => Promise.resolve(null)) }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

const project = {
  id: 'p1', name: 'Alpha', description: '', path: 'E:/Alpha', tags: [],
  cover_image: null, open_count: 0, last_opened_at: null, starred: false,
  created_at: '2024-01-01T00:00:00.000Z', last_opened_tools: [], workspace_tools: [],
};

describe('OrchestrateView — running derived from runId (F11: no stuck-true)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useNavigationStore.setState({
      activeProject: project,
      activeView: 'orchestrate',
      sidebarOpen: true,
      selectedConversationId: null,
    });
    // Reset the orchestrate store to its initial state (runId === null).
    useOrchestrateStore.setState({
      yaml: 'nodes: []',
      nodes: {},
      runId: null,
      output: null,
      error: null,
      pendingApproval: null,
    } as Partial<ReturnType<typeof useOrchestrateStore.getState>> as never);
  });

  it('按钮初始可点（running=false，因为 runId=null）', () => {
    render(<OrchestrateView />);
    const runBtn = screen.getByRole('button', { name: /运行/ });
    expect(runBtn).not.toBeDisabled();
  });

  it('run_id 落地后按钮被锁住（running 派生为 true）', async () => {
    // run_workflow resolves with a run_id; startRun flips store runId non-null.
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'run_workflow') return { run_id: 'r1', status: 'started' } as never;
      if (cmd === 'list_workflow_templates') return [] as never;
      return null as never;
    });

    render(<OrchestrateView />);
    const runBtn = screen.getByRole('button', { name: /运行/ });
    fireEvent.click(runBtn);

    // After the await resolves, startRun sets runId='r1' → running=true.
    await waitFor(() => {
      expect(useOrchestrateStore.getState().runId).toBe('r1');
    });
    // The button label flips to "运行中…" and resets to disabled.
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /运行中/ })).toBeDisabled();
    });
  });

  it('store 在 graph_failed 后 runId=null 即使后端断连不发事件——running 自动复位', async () => {
    // Pretend a run already started.
    useOrchestrateStore.setState({ runId: 'r1' } as never);
    render(<OrchestrateView />);
    expect(screen.getByRole('button', { name: /运行中/ })).toBeDisabled();

    // Simulate graph_failed: store flips runId back to null.
    useOrchestrateStore.getState().applyEvent({
      kind: 'graph_failed',
      error: 'backend panicked',
    } as never);

    // running derived from runId !== null → now false → button re-enabled.
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /运行/ })).not.toBeDisabled();
    });
  });

  it('reset 后按钮恢复可点（runId 派生单一真相源）', async () => {
    useOrchestrateStore.setState({ runId: 'r1' } as never);
    render(<OrchestrateView />);
    expect(screen.getByRole('button', { name: /运行中/ })).toBeDisabled();

    // reset() on the store clears runId.
    useOrchestrateStore.getState().reset();
    await waitFor(() => {
      expect(screen.getByRole('button', { name: /运行/ })).not.toBeDisabled();
    });
  });
});
