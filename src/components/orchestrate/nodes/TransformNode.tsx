import { Handle, Position } from '@xyflow/react';
import type { NodeProps, Node } from '@xyflow/react';
import type { DAGNodeData } from '../../../stores/orchestrateStore';

type TransformNodeType = Node<DAGNodeData, 'transform'>;

export function TransformNode({ data }: NodeProps<TransformNodeType>) {
  return (
    <div className="dag-node dag-node--transform">
      <Handle type="target" position={Position.Left} />
      <div className="dag-node__header">
        <span className="dag-node__icon">⚙️</span>
        <span className="dag-node__title">{data.label}</span>
      </div>
      <div className="dag-node__body">
        <span className="dag-node__hint">{(data.config.transformType as string) ?? 'Transform data'}</span>
      </div>
      <Handle type="source" position={Position.Right} />
    </div>
  );
}
