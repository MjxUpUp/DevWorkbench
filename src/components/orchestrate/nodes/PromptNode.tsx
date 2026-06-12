import { Handle, Position } from '@xyflow/react';
import type { NodeProps, Node } from '@xyflow/react';
import type { DAGNodeData } from '../../../stores/orchestrateStore';

type PromptNodeType = Node<DAGNodeData, 'prompt'>;

export function PromptNode({ data }: NodeProps<PromptNodeType>) {
  return (
    <div className="dag-node dag-node--prompt">
      <Handle type="target" position={Position.Left} />
      <div className="dag-node__header">
        <span className="dag-node__icon">💬</span>
        <span className="dag-node__title">{data.label}</span>
      </div>
      <div className="dag-node__body">
        <span className="dag-node__hint">{(data.config.prompt as string)?.slice(0, 40) ?? 'Prompt input'}</span>
      </div>
      <Handle type="source" position={Position.Right} />
    </div>
  );
}
