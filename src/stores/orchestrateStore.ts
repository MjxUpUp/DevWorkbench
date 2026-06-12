import { create } from 'zustand';
import type { Node, Edge, Connection } from '@xyflow/react';
import { applyNodeChanges, applyEdgeChanges, addEdge } from '@xyflow/react';
import type { NodeChange, EdgeChange } from '@xyflow/react';

export type DAGNodeType = 'prompt' | 'agent' | 'gate' | 'parallel' | 'merge' | 'human' | 'transform';

export interface DAGNodeData extends Record<string, unknown> {
  label: string;
  nodeType: DAGNodeType;
  config: Record<string, unknown>;
  status?: 'idle' | 'running' | 'success' | 'error';
}

interface OrchestrateState {
  nodes: Node<DAGNodeData>[];
  edges: Edge[];
  selectedNodeId: string | null;
  isRunning: boolean;
  workflowName: string;

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
}

export const useOrchestrateStore = create<OrchestrateState>((set, get) => ({
  nodes: [],
  edges: [],
  selectedNodeId: null,
  isRunning: false,
  workflowName: 'Untitled Workflow',

  setSelectedNodeId: (id) => set({ selectedNodeId: id }),
  setRunning: (running) => set({ isRunning: running }),
  setWorkflowName: (name) => set({ workflowName: name }),

  onNodesChange: (changes) => {
    set({ nodes: applyNodeChanges(changes, get().nodes) });
  },

  onEdgesChange: (changes) => {
    set({ edges: applyEdgeChanges(changes, get().edges) });
  },

  onConnect: (connection) => {
    set({ edges: addEdge(connection, get().edges) });
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

  clearCanvas: () => set({ nodes: [], edges: [], selectedNodeId: null }),

  loadFromYaml: (yaml) => {
    try {
      const data = JSON.parse(yaml);
      set({
        nodes: data.nodes ?? [],
        edges: data.edges ?? [],
        workflowName: data.workflowName ?? 'Untitled Workflow',
        selectedNodeId: null,
      });
    } catch {
      // invalid yaml/json, ignore
    }
  },

  exportToYaml: () => {
    const { nodes, edges, workflowName } = get();
    return JSON.stringify({ workflowName, nodes, edges }, null, 2);
  },
}));
