import { create } from 'zustand';
import { invoke } from '@tauri-apps/api/core';
import type { WorkflowProgressEvent } from '../types';

/** Per-node status surfaced from workflow:progress events. */
export type NodeState = {
  status: 'pending' | 'running' | 'done' | 'failed' | 'skipped' | 'waiting_approval';
  error?: string;
  /** Live-streamed agent output (accumulated `node_output` Delta chunks).
   * Populated only for agent nodes; null/undefined otherwise. */
  streamingOutput?: string;
};

interface OrchestrateState {
  /** The workflow YAML being edited / run. */
  yaml: string;
  /** node id -> status, updated live as events arrive. */
  nodes: Record<string, NodeState>;
  /** The active run id (null when idle). */
  runId: string | null;
  /** Final output once the graph completes. */
  output: unknown;
  /** Last error if the graph failed. */
  error: string | null;
  /** Approval prompts awaiting a decision (Human nodes). */
  pendingApproval: { node: string; prompt: string; resumeToken: string } | null;

  setYaml: (yaml: string) => void;
  /** Apply one progress event to the node map. */
  applyEvent: (event: WorkflowProgressEvent) => void;
  /** Start a run (resets node map, sets runId). */
  startRun: (runId: string) => void;
  /** Resolve the current pending approval (approve/reject a paused Human node). */
  approve: (approved: boolean) => Promise<void>;
  reset: () => void;
}

/** A default 3-node sample workflow so the view is usable on first open. */
export const SAMPLE_YAML = `start: prompt_1
end: gate_1
nodes:
  prompt_1:
    type: prompt
    text: "refactor the auth module and add tests"
  agent_1:
    type: agent
    agent: claude_code
    model: sonnet
  gate_1:
    type: gate
    gate: forge
edges:
  - { from: prompt_1, to: agent_1 }
  - { from: agent_1, to: gate_1 }
`;

export const useOrchestrateStore = create<OrchestrateState>((set, get) => ({
  yaml: SAMPLE_YAML,
  nodes: {},
  runId: null,
  output: null,
  error: null,
  pendingApproval: null,

  setYaml: (yaml) => set({ yaml }),

  applyEvent: (event) =>
    set((state) => {
      switch (event.kind) {
        case 'node_start':
          return {
            nodes: { ...state.nodes, [event.node]: { status: 'running' } },
          };
        case 'node_end':
          return {
            nodes: {
              ...state.nodes,
              [event.node]: {
                // Preserve streamingOutput (and any prior fields) — node_end
                // carries status, not the accumulated text.
                ...state.nodes[event.node],
                status: event.status,
                error: event.error,
              },
            },
          };
        case 'node_output': {
          // Gap-④ wiring: accumulate each streaming Delta chunk onto the node so
          // the canvas can render live agent output. The Final chunk arrives
          // separately via node_end/graph_done (the node's logical output).
          const prev = state.nodes[event.node];
          return {
            nodes: {
              ...state.nodes,
              [event.node]: {
                ...(prev ?? { status: 'running' as const }),
                streamingOutput: (prev?.streamingOutput ?? '') + formatChunk(event.chunk),
              },
            },
          };
        }
        case 'approval_required':
          return {
            nodes: { ...state.nodes, [event.node]: { status: 'waiting_approval' } },
            pendingApproval: {
              node: event.node,
              prompt: event.prompt,
              resumeToken: event.resume_token,
            },
          };
        case 'graph_done':
          return { output: event.output, runId: null, pendingApproval: null };
        case 'graph_failed':
          return { error: event.error, runId: null, pendingApproval: null };
        default:
          return state;
      }
    }),

  startRun: (runId) =>
    set({ nodes: {}, output: null, error: null, runId, pendingApproval: null }),

  reset: () =>
    set({ nodes: {}, output: null, error: null, runId: null, pendingApproval: null }),

  approve: async (approved) => {
    const { pendingApproval, runId } = get();
    if (!pendingApproval || !runId) return;
    try {
      await invoke('approve_workflow_step', {
        runId,
        resumeToken: pendingApproval.resumeToken,
        approved,
      });
      // Optimistically clear the prompt; the run continues and emits the
      // node_end / graph_done events that finalize state.
      set({ pendingApproval: null });
    } catch (e) {
      console.error('approve_workflow_step failed', e);
    }
  },
}));

/** Parse node ids out of the YAML so the canvas can show all nodes (even
 * pending ones) before the run starts. Best-effort line scan, not a full parser. */
export function parseNodeIds(yaml: string): string[] {
  const ids: string[] = [];
  let inNodes = false;
  for (const line of yaml.split('\n')) {
    if (/^nodes:/.test(line.trim())) {
      inNodes = true;
      continue;
    }
    if (inNodes) {
      // A node id line: "  node_id:" at 2-space indent, value is a map.
      const m = line.match(/^\s{2}(\w+):\s*$/);
      if (m) {
        ids.push(m[1]);
      } else if (/^\S/.test(line) && !line.trim().startsWith('#')) {
        // Dedent to top level => left nodes block.
        if (ids.length > 0) break;
      }
    }
  }
  return ids;
}

/** Render a `node_output` chunk to displayable text.
 *
 * The kernel emits two chunk shapes:
 * - Real agents (ReactAgent/OpaqueAgent): `AgentChunk::Delta(Value::String(t))`
 *   → the JSON value is a plain string.
 * - Test/mock executors: `AgentChunk::Delta(json!({"partial": ...}))` → an
 *   object with a `partial` field.
 *
 * Anything else falls back to JSON for visibility (never silently dropped). */
function formatChunk(chunk: unknown): string {
  if (typeof chunk === 'string') return chunk;
  if (chunk && typeof chunk === 'object' && 'partial' in (chunk as Record<string, unknown>)) {
    return String((chunk as { partial: unknown }).partial);
  }
  try {
    return JSON.stringify(chunk);
  } catch {
    return String(chunk);
  }
}
