import { Handle, Position } from '@xyflow/react';
import type { NodeProps, Node } from '@xyflow/react';
import type { DAGNodeData } from '../../../stores/orchestrateStore';

type AgentNodeType = Node<DAGNodeData, 'agent'>;

export function AgentNode({ data }: NodeProps<AgentNodeType>) {
  return (
    <div className="dag-node dag-node--agent">
      <Handle type="target" position={Position.Left} />
      <div className="dag-node__header">
        <span className="dag-node__icon">🤖</span>
        <span className="dag-node__title">{data.label}</span>
      </div>
      <div className="dag-node__body">
        <span className="dag-node__hint">{(data.config.agentType as string) ?? 'Select agent'}</span>
      </div>
      <Handle type="source" position={Position.Right} />
    </div>
  );
}
