import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useNavigationStore } from '../../stores/navigationStore';
import {
  parseNodeIds,
  useOrchestrateStore,
  type NodeState,
} from '../../stores/orchestrateStore';
import { BlocksView } from '../chat/BlocksView';
import type { ChatStreamEvent, WorkflowProgressPayload, WorkflowRunResult } from '../../types';

/** Color per node status — drives the canvas node fill. */
const STATUS_COLOR: Record<NodeState['status'], string> = {
  pending: 'var(--color-node-idle, #9ca3af)',
  running: 'var(--color-node-running, #3b82f6)',
  done: 'var(--color-node-done, #22c55e)',
  failed: 'var(--color-node-failed, #ef4444)',
  skipped: 'var(--color-node-skipped, #6b7280)',
  waiting_approval: 'var(--color-node-approval, #f59e0b)',
};

const STATUS_LABEL: Record<NodeState['status'], string> = {
  pending: '待执行',
  running: '执行中',
  done: '完成',
  failed: '失败',
  skipped: '已跳过',
  waiting_approval: '等待审批',
};

export function OrchestrateView() {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const yaml = useOrchestrateStore((s) => s.yaml);
  const setYaml = useOrchestrateStore((s) => s.setYaml);
  const nodes = useOrchestrateStore((s) => s.nodes);
  const runId = useOrchestrateStore((s) => s.runId);
  const output = useOrchestrateStore((s) => s.output);
  const error = useOrchestrateStore((s) => s.error);
  const pendingApproval = useOrchestrateStore((s) => s.pendingApproval);
  const applyEvent = useOrchestrateStore((s) => s.applyEvent);
  const approve = useOrchestrateStore((s) => s.approve);
  const startRun = useOrchestrateStore((s) => s.startRun);
  const reset = useOrchestrateStore((s) => s.reset);

  const [running, setRunning] = useState(false);
  const [eventLog, setEventLog] = useState<string[]>([]);

  // Subscribe to workflow:progress once. Guard against the unmount-before-
  // resolve race: if the component unmounts while the async listen() is still
  // pending, the cleanup runs with unlisten still null and the listener leaks.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    (async () => {
      const fn = await listen<WorkflowProgressPayload>('workflow:progress', (e) => {
        const { event } = e.payload;
        applyEvent(event);
        setEventLog((prev) => [...prev.slice(-50), formatEvent(event)]);
      });
      if (cancelled) {
        fn(); // already unmounted — clean up immediately
      } else {
        unlisten = fn;
      }
    })();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, [applyEvent]);

  const nodeIds = parseNodeIds(yaml);
  const runningNode = Object.entries(nodes).find(([, s]) => s.status === 'running')?.[0];

  const handleRun = async () => {
    setRunning(true);
    setEventLog([]);
    reset();
    try {
      const result = await invoke<WorkflowRunResult>('run_workflow', {
        yamlContent: yaml,
        input: { task: 'orchestrate run' },
        workingDir: activeProject?.path ?? null,
      });
      startRun(result.run_id);
    } catch (e) {
      setEventLog((prev) => [...prev, `[error] ${String(e)}`]);
    } finally {
      setRunning(false);
    }
  };

  return (
    <div className="orchestrate-view">
      <header className="orchestrate-header">
        <h2>DAG 编排</h2>
        <div className="orchestrate-actions">
          <span className="orchestrate-project">
            {activeProject ? activeProject.name : '未选项目'}
          </span>
          <button
            className="btn btn-primary"
            onClick={handleRun}
            disabled={running || !activeProject}
            title={!activeProject ? '请先选择项目' : ''}
          >
            {running ? '运行中…' : '运行 Workflow'}
          </button>
          <button className="btn" onClick={reset} disabled={running}>
            重置
          </button>
        </div>
      </header>

      <div className="orchestrate-body">
        {/* YAML editor */}
        <section className="orchestrate-yaml">
          <h3>Workflow 定义 (YAML)</h3>
          <textarea
            value={yaml}
            onChange={(e) => setYaml(e.target.value)}
            spellCheck={false}
            className="yaml-editor"
          />
        </section>

        {/* Canvas — nodes light up by status */}
        <section className="orchestrate-canvas">
          <h3>节点状态 {runningNode ? `(运行中: ${runningNode})` : ''}</h3>
          <div className="node-list">
            {nodeIds.length === 0 && <p className="muted">YAML 中暂无节点</p>}
            {nodeIds.map((id) => {
              const state = nodes[id] ?? { status: 'pending' as const };
              return (
                <div
                  key={id}
                  className={`dag-node dag-node--${state.status}`}
                  style={{ borderLeftColor: STATUS_COLOR[state.status] }}
                >
                  <span className="dag-node-id">{id}</span>
                  <span
                    className="dag-node-dot"
                    style={{ background: STATUS_COLOR[state.status] }}
                  />
                  <span className="dag-node-status">{STATUS_LABEL[state.status]}</span>
                  {state.blocks && state.blocks.length > 0 && (
                    <div className="dag-node-stream">
                      <BlocksView events={state.blocks} running={state.status === 'running'} />
                    </div>
                  )}
                  {state.error && <pre className="dag-node-error">{state.error}</pre>}
                </div>
              );
            })}
          </div>

          {pendingApproval && (
            <div className="approval-card">
              <strong>需要审批: {pendingApproval.node}</strong>
              <p>{pendingApproval.prompt}</p>
              <div className="approval-actions">
                <button className="btn btn-primary" onClick={() => approve(true)}>批准</button>
                <button className="btn" onClick={() => approve(false)}>拒绝</button>
              </div>
            </div>
          )}

          {output != null && (
            <div className="graph-output">
              <h4>最终输出</h4>
              <pre>{JSON.stringify(output, null, 2)}</pre>
            </div>
          )}
          {error && <div className="graph-error">失败: {error}</div>}
        </section>

        {/* Event log */}
        <section className="orchestrate-log">
          <h3>事件流{runId ? ` · ${runId.slice(0, 8)}` : ''}</h3>
          <div className="event-log">
            {eventLog.length === 0 && <p className="muted">尚无事件</p>}
            {eventLog.map((line, i) => (
              <div key={i} className="event-line">
                {line}
              </div>
            ))}
          </div>
        </section>
      </div>
    </div>
  );
}

function formatEvent(event: WorkflowProgressPayload['event']): string {
  switch (event.kind) {
    case 'node_start':
      return `▶ ${event.node} 开始`;
    case 'node_end':
      return `${event.status === 'done' ? '✓' : event.status === 'failed' ? '✗' : '⊘'} ${event.node} ${event.error ? '— ' + event.error : ''}`;
    case 'graph_done':
      return `■ workflow 完成`;
    case 'graph_failed':
      return `■ workflow 失败: ${event.error}`;
    case 'approval_required':
      return `? ${event.node} 等待审批`;
    case 'node_output': {
      // Real workflow chunks are now ChatStreamEvent (kind discriminator);
      // test/mock executors still emit {partial}. Preview per kind so the event
      // log reflects the structure (text / 🔧 tool / result) instead of a raw
      // JSON blob.
      const c = event.chunk as unknown;
      let text: string;
      if (c && typeof c === 'object' && 'kind' in c) {
        const ev = c as ChatStreamEvent;
        switch (ev.kind) {
          case 'text': text = ev.content; break;
          case 'tool_use': text = `🔧 ${ev.name}`; break;
          case 'tool_result': text = ev.content; break;
          case 'result': text = ev.is_error ? '✗ 失败' : '✓ 完成'; break;
          case 'thinking': text = `💭 ${ev.content.slice(0, 40)}`; break;
          default: text = JSON.stringify(c);
        }
      } else if (typeof c === 'string') {
        text = c;
      } else if (c && typeof c === 'object' && 'partial' in c) {
        text = String((c as { partial: unknown }).partial);
      } else {
        text = JSON.stringify(c);
      }
      const preview = text.length > 80 ? `${text.slice(0, 80)}…` : text;
      return `  ▸ ${event.node}: ${preview}`;
    }
    default: {
      // Exhaustiveness guard: if a new WorkflowProgressEvent kind is added
      // without a case above, `event` is no longer `never` and this line
      // errors — forcing the author to handle it instead of silently falling
      // through. Previously this read `event.kind`, which only compiled when
      // the switch was *not* exhaustive.
      const _exhaustive: never = event;
      return `· unhandled: ${JSON.stringify(_exhaustive)}`;
    }
  }
}
