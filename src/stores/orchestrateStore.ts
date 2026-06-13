import { create } from 'zustand';
import type { Node, Edge, Connection } from '@xyflow/react';
import { applyNodeChanges, applyEdgeChanges, addEdge } from '@xyflow/react';
import type { NodeChange, EdgeChange } from '@xyflow/react';
import yaml from 'js-yaml';
import { invoke } from '@tauri-apps/api/core';
import type { Workflow } from '../types';

export type DAGNodeType = 'prompt' | 'agent' | 'gate' | 'parallel' | 'merge' | 'human' | 'transform';

export interface DAGNodeData extends Record<string, unknown> {
  label: string;
  nodeType: DAGNodeType;
  config: Record<string, unknown>;
  status?: 'idle' | 'running' | 'success' | 'error' | 'blocked';
}

interface OrchestrateState {
  nodes: Node<DAGNodeData>[];
  edges: Edge[];
  selectedNodeId: string | null;
  isRunning: boolean;
  workflowName: string;
  workflowId: string | null;
  workflowList: Workflow[];
  loading: boolean;

  setSelectedNodeId: (id: string | null) => void;
  setRunning: (running: boolean) => void;
  setWorkflowName: (name: string) => void;
  onNodesChange: (changes: NodeChange<Node<DAGNodeData>>[]) => void;
  onEdgesChange: (changes: EdgeChange[]) => void;
  onConnect: (connection: Connection) => void;
  addNode: (node: Node<DAGNodeData>) => void;
  updateNode: (id: string, data: Partial<DAGNodeData>) => void;
  removeNode: (id: string) => void;
  setEdges: (edges: Edge[]) => void;
  clearCanvas: () => void;
  loadFromYaml: (yaml: string) => void;
  exportToYaml: () => string;

  // Persistence actions
  listWorkflows: () => Promise<void>;
  saveWorkflow: () => Promise<void>;
  loadWorkflow: (id: string) => Promise<void>;
  deleteWorkflow: (id: string) => Promise<void>;
}

export const useOrchestrateStore = create<OrchestrateState>((set, get) => ({
  nodes: [],
  edges: [],
  selectedNodeId: null,
  isRunning: false,
  workflowName: 'Untitled Workflow',
  workflowId: null,
  workflowList: [],
  loading: false,

  setSelectedNodeId: (id) => set({ selectedNodeId: id }),
  setRunning: (running) => set({ isRunning: running }),
  setWorkflowName: (name) => set({ workflowName: name }),

  onNodesChange: (changes) => {
    const newNodes = applyNodeChanges(changes, get().nodes);
    const removedIds = new Set(
      changes.filter((c) => c.type === 'remove').map((c) => c.id)
    );
    if (removedIds.size > 0) {
      set({
        nodes: newNodes,
        edges: get().edges.filter((e) => !removedIds.has(e.source) && !removedIds.has(e.target)),
      });
    } else {
      set({ nodes: newNodes });
    }
  },

  onEdgesChange: (changes) => {
    set({ edges: applyEdgeChanges(changes, get().edges) });
  },

  onConnect: (connection) => {
    set({ edges: addEdge({ ...connection, label: 'success' }, get().edges) });
  },

  addNode: (node) => {
    set({ nodes: [...get().nodes, node] });
  },

  updateNode: (id, data) => {
    set({
      nodes: get().nodes.map((n) =>
        n.id === id ? { ...n, data: { ...n.data, ...data } } : n
      ),
    });
  },

  removeNode: (id) => {
    set({
      nodes: get().nodes.filter((n) => n.id !== id),
      edges: get().edges.filter((e) => e.source !== id && e.target !== id),
      selectedNodeId: get().selectedNodeId === id ? null : get().selectedNodeId,
    });
  },

  setEdges: (edges) => set({ edges }),

  clearCanvas: () => set({ nodes: [], edges: [], selectedNodeId: null, workflowId: null }),

  loadFromYaml: (yamlStr) => {
    try {
      const data = yaml.load(yamlStr) as { workflowName?: string; nodes?: Node<DAGNodeData>[]; edges?: Edge[] };
      set({
        nodes: data.nodes ?? [],
        edges: data.edges ?? [],
        workflowName: data.workflowName ?? 'Untitled Workflow',
        selectedNodeId: null,
      });
    } catch {
      // invalid yaml, ignore
    }
  },

  exportToYaml: () => {
    const { nodes, edges, workflowName } = get();
    return yaml.dump({ workflowName, nodes, edges }, { skipInvalid: true });
  },

  // ---- Persistence ----

  listWorkflows: async () => {
    try {
      const list = await invoke<Workflow[]>('list_workflows');
      set({ workflowList: list });
    } catch (e) {
      console.error('Failed to list workflows:', e);
    }
  },

  saveWorkflow: async () => {
    const { workflowName, workflowId } = get();
    const yamlContent = get().exportToYaml();
    set({ loading: true });
    try {
      if (workflowId) {
        const updated = await invoke<Workflow>('update_workflow', {
          id: workflowId,
          name: workflowName,
          yamlContent,
        });
        set({ workflowId: updated.id, loading: false });
      } else {
        const created = await invoke<Workflow>('create_workflow', {
          name: workflowName,
          yamlContent,
        });
        set({ workflowId: created.id, loading: false });
      }
      // Refresh list
      await get().listWorkflows();
    } catch (e) {
      console.error('Failed to save workflow:', e);
      set({ loading: false });
    }
  },

  loadWorkflow: async (id) => {
    set({ loading: true });
    try {
      const wf = await invoke<Workflow>('get_workflow', { id });
      get().loadFromYaml(wf.yamlContent);
      set({ workflowId: wf.id, workflowName: wf.name, loading: false });
    } catch (e) {
      console.error('Failed to load workflow:', e);
      set({ loading: false });
    }
  },

  deleteWorkflow: async (id) => {
    try {
      await invoke('delete_workflow', { id });
      // If we deleted the current workflow, reset
      if (get().workflowId === id) {
        set({ workflowId: null });
      }
      await get().listWorkflows();
    } catch (e) {
      console.error('Failed to delete workflow:', e);
    }
  },
}));
