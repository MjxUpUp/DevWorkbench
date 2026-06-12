import { useOrchestrateStore } from '../../stores/orchestrateStore';

const NODE_TYPE_LABELS: Record<string, string> = {
  prompt: '💬 Prompt',
  agent: '🤖 Agent',
  gate: '🛡️ Gate',
  parallel: '⫸ Parallel',
  merge: '⫷ Merge',
  human: '👤 Human',
  transform: '⚙️ Transform',
};

export function NodeDetailPanel() {
  const selectedNodeId = useOrchestrateStore((s) => s.selectedNodeId);
  const nodes = useOrchestrateStore((s) => s.nodes);
  const updateNode = useOrchestrateStore((s) => s.updateNode);

  const selectedNode = nodes.find((n) => n.id === selectedNodeId);

  if (!selectedNode) return null;

  const nodeId = selectedNode.id;
  const { data } = selectedNode;

  function handleLabelChange(newLabel: string) {
    updateNode(nodeId, { label: newLabel });
  }

  function handleConfigChange(key: string, value: string) {
    updateNode(nodeId, {
      config: { ...data.config, [key]: value },
    });
  }

  return (
    <div className="node-detail-panel">
      <div className="node-detail-panel__header">
        <span className="node-detail-panel__type">
          {NODE_TYPE_LABELS[data.nodeType] ?? data.nodeType}
        </span>
        <span className="node-detail-panel__status">
          {data.status ?? 'idle'}
        </span>
      </div>
      <div className="node-detail-panel__fields">
        <label className="node-detail-panel__field">
          <span className="node-detail-panel__field-label">Label</span>
          <input
            className="node-detail-panel__input"
            value={data.label}
            onChange={(e) => handleLabelChange(e.target.value)}
          />
        </label>
        {renderTypeSpecificFields(data.nodeType, data.config, handleConfigChange)}
      </div>
    </div>
  );
}

function renderTypeSpecificFields(
  nodeType: string,
  config: Record<string, unknown>,
  onChange: (key: string, value: string) => void,
) {
  switch (nodeType) {
    case 'prompt':
      return (
        <label className="node-detail-panel__field">
          <span className="node-detail-panel__field-label">Prompt Text</span>
          <textarea
            className="node-detail-panel__textarea"
            value={(config.prompt as string) ?? ''}
            onChange={(e) => onChange('prompt', e.target.value)}
          />
        </label>
      );
    case 'agent':
      return (
        <>
          <label className="node-detail-panel__field">
            <span className="node-detail-panel__field-label">Agent Type</span>
            <select
              className="node-detail-panel__select"
              value={(config.agentType as string) ?? ''}
              onChange={(e) => onChange('agentType', e.target.value)}
            >
              <option value="">Select agent</option>
              <option value="coder">Coder</option>
              <option value="reviewer">Reviewer</option>
              <option value="architect">Architect</option>
            </select>
          </label>
          <label className="node-detail-panel__field">
            <span className="node-detail-panel__field-label">Model</span>
            <input
              className="node-detail-panel__input"
              value={(config.model as string) ?? ''}
              onChange={(e) => onChange('model', e.target.value)}
              placeholder="e.g. claude-sonnet-4-20250514"
            />
          </label>
        </>
      );
    case 'gate':
      return (
        <>
          <label className="node-detail-panel__field">
            <span className="node-detail-panel__field-label">Gate Type</span>
            <select
              className="node-detail-panel__select"
              value={(config.gateType as string) ?? ''}
              onChange={(e) => onChange('gateType', e.target.value)}
            >
              <option value="">Select gate</option>
              <option value="lint">Lint Pass</option>
              <option value="test">Test Pass</option>
              <option value="review">Code Review</option>
              <option value="security">Security Scan</option>
            </select>
          </label>
          <label className="node-detail-panel__field">
            <span className="node-detail-panel__field-label">Threshold</span>
            <input
              className="node-detail-panel__input"
              value={(config.threshold as string) ?? ''}
              onChange={(e) => onChange('threshold', e.target.value)}
              placeholder="e.g. 80%"
            />
          </label>
        </>
      );
    case 'human':
      return (
        <label className="node-detail-panel__field">
          <span className="node-detail-panel__field-label">Approver</span>
          <input
            className="node-detail-panel__input"
            value={(config.approver as string) ?? ''}
            onChange={(e) => onChange('approver', e.target.value)}
            placeholder="e.g. tech-lead"
          />
        </label>
      );
    case 'transform':
      return (
        <label className="node-detail-panel__field">
          <span className="node-detail-panel__field-label">Transform Type</span>
          <select
            className="node-detail-panel__select"
            value={(config.transformType as string) ?? ''}
            onChange={(e) => onChange('transformType', e.target.value)}
          >
            <option value="">Select transform</option>
            <option value="filter">Filter</option>
            <option value="map">Map</option>
            <option value="aggregate">Aggregate</option>
            <option value="template">Template</option>
          </select>
        </label>
      );
    case 'parallel':
    case 'merge':
      return (
        <label className="node-detail-panel__field">
          <span className="node-detail-panel__field-label">Branch Count</span>
          <input
            className="node-detail-panel__input"
            type="number"
            min={2}
            value={(config.branchCount as string) ?? '2'}
            onChange={(e) => onChange('branchCount', e.target.value)}
          />
        </label>
      );
    default:
      return null;
  }
}
