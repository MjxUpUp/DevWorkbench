import { useEffect, useState } from 'react';
import { listen } from '@tauri-apps/api/event';
import styles from './BlocksView.module.css';

/** Raw GraphEvent (kernel-compose events.rs) tagged shape, as emitted by the
 *  WorkflowTool's `workflow_graph:progress` payload. Only the fields the strip
 *  reads are typed; unknown kinds are ignored. Matches the Rust
 *  `#[serde(tag="kind", rename_all="snake_case")]` wire format. */
interface GraphProgressEvent {
  kind: string;
  node?: string;
  status?: string;
  attempt?: number;
  error?: string;
}
interface WorkflowGraphPayload {
  run_id: string;
  event: GraphProgressEvent;
}

interface NodeState {
  status: string;
  retries: number;
  error?: string;
}

const SETTLED_KINDS = new Set(['graph_done', 'graph_failed', 'graph_interrupted']);

/** Live node-status strip rendered alongside a `run_workflow_graph` tool_use
 *  pill while the orchestrator-authored DAG executes. Subscribes to the
 *  `workflow_graph:progress` Tauri event and lights up one chip per node as
 *  node_start / node_end / node_retried arrive.
 *
 *  Association: the orchestrator blocks on each run_workflow_graph call, so at
 *  most one workflow runs at a time — the latest event stream IS the current
 *  tool_use. No per-message run_id wiring is needed (and none is possible: the
 *  tool generates run_id internally, the chat tool_use never carries it).
 *
 *  Lifecycle: renders nothing until the first node appears; hides itself once
 *  the graph settles (graph_done/failed/interrupted) — the subsequent
 *  tool_result pill (format_outcome, with the full status table + retry
 *  history) takes over as the persistent record. So this strip is purely the
 *  live view, never the record. */
export function WorkflowProgressStrip() {
  const [nodes, setNodes] = useState<Record<string, NodeState>>({});
  const [settled, setSettled] = useState(false);

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    (async () => {
      const fn = await listen<WorkflowGraphPayload>('workflow_graph:progress', (e) => {
        const ev = e.payload.event;
        setNodes((prev) => {
          if (!ev.node) return prev;
          const cur = prev[ev.node] ?? { status: 'pending', retries: 0 };
          const next = { ...prev };
          switch (ev.kind) {
            case 'node_start':
              next[ev.node] = { ...cur, status: 'running' };
              break;
            case 'node_end':
              next[ev.node] = { ...cur, status: ev.status ?? 'done', error: ev.error };
              break;
            case 'node_retried':
              next[ev.node] = { ...cur, status: 'running', retries: cur.retries + 1 };
              break;
            default:
              break;
          }
          return next;
        });
        if (SETTLED_KINDS.has(ev.kind)) setSettled(true);
      });
      if (cancelled) fn();
      else unlisten = fn;
    })();
    return () => {
      cancelled = true;
      if (unlisten) unlisten();
    };
  }, []);

  if (settled) return null;
  const ids = Object.keys(nodes);
  if (ids.length === 0) return null;

  return (
    <div
      className={styles.workflowStrip}
      data-testid="workflow-progress-strip"
      role="status"
      aria-live="polite"
      aria-label="自规划工作流执行进度"
    >
      {ids.map((id) => {
        const st = nodes[id];
        return (
          <span
            key={id}
            className={`${styles.workflowChip} ${STATUS_CLASS[st.status] ?? ''}`}
            title={st.error ? `${id}：${st.error}` : id}
          >
            <span aria-hidden="true">{chipIcon(st.status)}</span>
            {id}
            {st.retries > 0 && <span className={styles.wfRetry} aria-label={`重试 ${st.retries} 次`}> ⟳{st.retries}</span>}
          </span>
        );
      })}
    </div>
  );
}

const STATUS_CLASS: Record<string, string> = {
  done: styles.wfDone,
  failed: styles.wfFailed,
  running: styles.wfRunning,
  pending: styles.wfPending,
  skipped: styles.wfSkipped,
  interrupted: styles.wfInterrupted,
};

function chipIcon(status: string): string {
  switch (status) {
    case 'done':
      return '✓';
    case 'failed':
      return '✗';
    case 'skipped':
      return '–';
    case 'interrupted':
      return '■';
    default:
      return '▸'; // running / pending / retried-transient
  }
}
