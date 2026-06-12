import { useCallback, useRef } from 'react';
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  BackgroundVariant,
  type ReactFlowInstance,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';

import { useOrchestrateStore } from '../../stores/orchestrateStore';
import type { DAGNodeData, DAGNodeType } from '../../stores/orchestrateStore';
import type { Node } from '@xyflow/react';

import { PromptNode } from './nodes/PromptNode';
import { AgentNode } from './nodes/AgentNode';
import { GateNode } from './nodes/GateNode';
import { ParallelNode } from './nodes/ParallelNode';
import { MergeNode } from './nodes/MergeNode';
import { HumanNode } from './nodes/HumanNode';
import { TransformNode } from './nodes/TransformNode';

const NODE_TYPES = {
  prompt: PromptNode,
  agent: AgentNode,
  gate: GateNode,
  parallel: ParallelNode,
  merge: MergeNode,
  human: HumanNode,
  transform: TransformNode,
};

const NODE_DEFAULTS: Record<DAGNodeType, { label: string; icon: string }> = {
  prompt: { label: 'Prompt', icon: '💬' },
  agent: { label: 'Agent', icon: '🤖' },
  gate: { label: 'Gate', icon: '🛡️' },
  parallel: { label: 'Parallel', icon: '⫸' },
  merge: { label: 'Merge', icon: '⫷' },
  human: { label: 'Human', icon: '👤' },
  transform: { label: 'Transform', icon: '⚙️' },
};

let nodeIdCounter = 0;

export function FlowCanvas() {
  const nodes = useOrchestrateStore((s) => s.nodes);
  const edges = useOrchestrateStore((s) => s.edges);
  const onNodesChange = useOrchestrateStore((s) => s.onNodesChange);
  const onEdgesChange = useOrchestrateStore((s) => s.onEdgesChange);
  const onConnect = useOrchestrateStore((s) => s.onConnect);
  const addNode = useOrchestrateStore((s) => s.addNode);
  const setSelectedNodeId = useOrchestrateStore((s) => s.setSelectedNodeId);

  const reactFlowInstance = useRef<ReactFlowInstance<Node<DAGNodeData>> | null>(null);

  const onInit = useCallback((instance: ReactFlowInstance<Node<DAGNodeData>>) => {
    reactFlowInstance.current = instance;
  }, []);

  const onDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault();
    e.dataTransfer.dropEffect = 'copy';
  }, []);

  const onDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault();
      const nodeType = e.dataTransfer.getData('application/dag-node-type') as DAGNodeType;
      if (!nodeType || !NODE_DEFAULTS[nodeType]) return;

      const instance = reactFlowInstance.current;
      if (!instance) return;

      const position = instance.screenToFlowPosition({
        x: e.clientX,
        y: e.clientY,
      });

      const defaults = NODE_DEFAULTS[nodeType];
      const id = `${nodeType}_${++nodeIdCounter}_${Date.now()}`;

      const newNode: Node<DAGNodeData> = {
        id,
        type: nodeType,
        position,
        data: {
          label: defaults.label,
          nodeType,
          config: {},
          status: 'idle',
        },
      };

      addNode(newNode);
    },
    [addNode],
  );

  const onNodeClick = useCallback(
    (_: React.MouseEvent, node: Node<DAGNodeData>) => {
      setSelectedNodeId(node.id);
    },
    [setSelectedNodeId],
  );

  const onPaneClick = useCallback(() => {
    setSelectedNodeId(null);
  }, [setSelectedNodeId]);

  return (
    <div className="flow-canvas">
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        onInit={onInit}
        onDrop={onDrop}
        onDragOver={onDragOver}
        onNodeClick={onNodeClick}
        onPaneClick={onPaneClick}
        nodeTypes={NODE_TYPES}
        fitView
        deleteKeyCode="Delete"
        className="flow-canvas__react-flow"
      >
        <Background variant={BackgroundVariant.Dots} gap={16} size={1} />
        <Controls position="bottom-right" />
        <MiniMap
          position="bottom-left"
          maskColor="rgba(0, 0, 0, 0.05)"
          style={{ borderRadius: 'var(--radius-md)' }}
        />
      </ReactFlow>
    </div>
  );
}
