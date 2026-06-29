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

describe('OrchestrateView — 加载统一 agent/模型管理(打通节点下拉数据源)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useNavigationStore.setState({
      activeProject: project,
      activeView: 'orchestrate',
      sidebarOpen: true,
      selectedConversationId: null,
    });
    useOrchestrateStore.setState({
      yaml: 'nodes: []',
      nodes: {},
      runId: null,
      output: null,
      error: null,
      pendingApproval: null,
    } as Partial<ReturnType<typeof useOrchestrateStore.getState>> as never);
  });

  it('挂载即触发 discover_agents_cmd + get_providers_config,节点 agent/model 下拉拿到统一数据', async () => {
    // WorkflowBuilder reads useAgentStore.agents + useProvidersStore.config for
    // the agent-node inspector dropdowns. Both stores are loaded by ChatView /
    // Settings only — entering orchestrate directly left them empty. The fix
    // fires refreshAgents() + loadProviders() on mount.
    vi.mocked(invoke).mockImplementation(async (cmd: string) => {
      if (cmd === 'discover_agents_cmd') return [] as never;
      if (cmd === 'get_providers_config') return { providers: [] } as never;
      return null as never;
    });

    render(<OrchestrateView />);

    await waitFor(() => {
      const cmds = vi.mocked(invoke).mock.calls.map(([c]) => c as string);
      expect(cmds).toContain('discover_agents_cmd');
      expect(cmds).toContain('get_providers_config');
    });
  });
});

describe('OrchestrateView — B4 workflow 持久化 CRUD 接通', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useNavigationStore.setState({
      activeProject: project,
      activeView: 'orchestrate',
      sidebarOpen: true,
      selectedConversationId: null,
    });
    useOrchestrateStore.setState({
      yaml: 'nodes: []',
      nodes: {},
      runId: null,
      output: null,
      error: null,
      pendingApproval: null,
      currentWorkflowId: null,
      savedWorkflows: [],
    } as Partial<ReturnType<typeof useOrchestrateStore.getState>> as never);
  });

  it('首次保存走 create_workflow，落库后 currentWorkflowId 置位', async () => {
    const created = { id: 'wf-1', name: '我的流程', yamlContent: 'nodes: []', createdAt: 't', updatedAt: 't' };
    vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === 'create_workflow') {
        // yamlContent 经 WorkflowBuilder 双向绑定规范化，不断言精确值；只验 name
        expect((args as { name: string }).name).toBe('我的流程');
        return created as never;
      }
      if (cmd === 'list_workflows') return [created] as never;
      if (cmd === 'list_workflow_templates') return [] as never;
      if (cmd === 'discover_agents_cmd') return [] as never;
      if (cmd === 'get_providers_config') return { providers: [] } as never;
      return null as never;
    });
    render(<OrchestrateView />);

    // currentWorkflowId === null → 按钮文案「保存为…」，点击打开对话框
    fireEvent.click(screen.getByRole('button', { name: '保存为…' }));
    const input = await screen.findByPlaceholderText('工作流名称');
    fireEvent.change(input, { target: { value: '我的流程' } });
    fireEvent.click(screen.getByRole('button', { name: '确认保存' }));

    await waitFor(() => {
      expect(useOrchestrateStore.getState().currentWorkflowId).toBe('wf-1');
    });
    expect(vi.mocked(invoke)).toHaveBeenCalledWith(
      'create_workflow',
      expect.objectContaining({ name: '我的流程' }),
    );
  });

  it('已存工作流（currentWorkflowId 非空）保存走 update_workflow 覆盖', async () => {
    useOrchestrateStore.setState({
      currentWorkflowId: 'wf-1',
      savedWorkflows: [{ id: 'wf-1', name: '旧名', yamlContent: 'x', createdAt: 't', updatedAt: 't' }],
    } as never);
    vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === 'update_workflow') {
        expect((args as { id: string }).id).toBe('wf-1');
        return { id: 'wf-1', name: '旧名', yamlContent: 'x', createdAt: 't', updatedAt: 't2' } as never;
      }
      if (cmd === 'list_workflows') return [] as never;
      if (cmd === 'list_workflow_templates') return [] as never;
      if (cmd === 'discover_agents_cmd') return [] as never;
      if (cmd === 'get_providers_config') return { providers: [] } as never;
      return null as never;
    });
    render(<OrchestrateView />);

    // currentWorkflowId 非空 → 按钮文案「保存」，直接 update（无对话框）
    fireEvent.click(screen.getByRole('button', { name: '保存' }));
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith(
        'update_workflow',
        expect.objectContaining({ id: 'wf-1', name: '旧名' }),
      ),
    );
  });

  it('历史 tab 渲染已保存列表并支持载入/删除', async () => {
    const wf = { id: 'wf-1', name: '已存流程', yamlContent: 'start: a', createdAt: 't', updatedAt: '2026-06-29T00:00:00Z' };
    vi.mocked(invoke).mockImplementation(async (cmd: string, args?: unknown) => {
      if (cmd === 'list_workflows') return [wf] as never;
      if (cmd === 'delete_workflow') {
        expect((args as { id: string }).id).toBe('wf-1');
        return null as never;
      }
      if (cmd === 'list_workflow_templates') return [] as never;
      if (cmd === 'discover_agents_cmd') return [] as never;
      if (cmd === 'get_providers_config') return { providers: [] } as never;
      return null as never;
    });
    render(<OrchestrateView />);

    // 切到历史 tab → 触发 list_workflows
    fireEvent.click(screen.getByRole('button', { name: '历史' }));
    expect(await screen.findByText('已存流程')).toBeInTheDocument();

    // 载入 → currentWorkflowId 置位；yaml 经 WorkflowBuilder 规范化但以载入的
    // start 节点开头，证明 wf.yamlContent 被注入编辑器
    fireEvent.click(screen.getByRole('button', { name: '载入' }));
    await waitFor(() => {
      expect(useOrchestrateStore.getState().yaml).toContain('start: a');
      expect(useOrchestrateStore.getState().currentWorkflowId).toBe('wf-1');
    });

    // 删除 → delete_workflow
    fireEvent.click(screen.getByRole('button', { name: '删除' }));
    await waitFor(() =>
      expect(vi.mocked(invoke)).toHaveBeenCalledWith('delete_workflow', { id: 'wf-1' }),
    );
  });
});
