import { ReactFlowProvider } from '@xyflow/react';
import { useOrchestrateStore } from '../../stores/orchestrateStore';
import { NodePalette } from './NodePalette';
import { FlowCanvas } from './FlowCanvas';
import { NodeDetailPanel } from './NodeDetailPanel';

interface TemplateCard {
  name: string;
  description: string;
  icon: string;
}

const TEMPLATES: TemplateCard[] = [
  {
    name: 'Code Review Pipeline',
    description: 'Automated code review with lint, test, and human approval gates',
    icon: '🔍',
  },
  {
    name: 'Bug Fix Pipeline',
    description: 'Structured bug fixing with analysis, fix, and verification steps',
    icon: '🐛',
  },
  {
    name: 'Full Feature',
    description: 'End-to-end feature development with parallel implementation tracks',
    icon: '🚀',
  },
];

export function OrchestrateView() {
  return (
    <ReactFlowProvider>
      <OrchestrateLayout />
    </ReactFlowProvider>
  );
}

function OrchestrateLayout() {
  const nodes = useOrchestrateStore((s) => s.nodes);
  const selectedNodeId = useOrchestrateStore((s) => s.selectedNodeId);
  const isRunning = useOrchestrateStore((s) => s.isRunning);
  const workflowName = useOrchestrateStore((s) => s.workflowName);
  const setRunning = useOrchestrateStore((s) => s.setRunning);
  const setWorkflowName = useOrchestrateStore((s) => s.setWorkflowName);
  const clearCanvas = useOrchestrateStore((s) => s.clearCanvas);
  const exportToYaml = useOrchestrateStore((s) => s.exportToYaml);

  const isEmpty = nodes.length === 0;

  return (
    <div className="orchestrate-view">
      <div className="orchestrate-toolbar">
        <div className="orchestrate-toolbar__left">
          <input
            className="orchestrate-toolbar__name"
            value={workflowName}
            onChange={(e) => setWorkflowName(e.target.value)}
          />
        </div>
        <div className="orchestrate-toolbar__actions">
          <button
            className="orchestrate-toolbar__btn"
            title="Save workflow"
            onClick={() => {
              const yaml = exportToYaml();
              console.log('Saved workflow:', yaml);
            }}
          >
            💾 Save
          </button>
          <button
            className="orchestrate-toolbar__btn orchestrate-toolbar__btn--run"
            title="Run workflow"
            disabled={isRunning || isEmpty}
            onClick={() => setRunning(true)}
          >
            ▶ Run
          </button>
          <button
            className="orchestrate-toolbar__btn orchestrate-toolbar__btn--stop"
            title="Stop workflow"
            disabled={!isRunning}
            onClick={() => setRunning(false)}
          >
            ⏹ Stop
          </button>
          <button
            className="orchestrate-toolbar__btn"
            title="Export YAML"
            onClick={() => {
              const yaml = exportToYaml();
              navigator.clipboard.writeText(yaml);
            }}
          >
            📋 YAML
          </button>
          <button
            className="orchestrate-toolbar__btn orchestrate-toolbar__btn--danger"
            title="Clear canvas"
            onClick={clearCanvas}
          >
            🗑️
          </button>
        </div>
      </div>

      <div className="orchestrate-body">
        <NodePalette />

        <div className="orchestrate-canvas-area">
          {isEmpty ? (
            <div className="orchestrate-empty">
              <div className="orchestrate-empty__title">Create a Workflow</div>
              <div className="orchestrate-empty__hint">
                Drag nodes from the palette or start from a template
              </div>
              <div className="orchestrate-templates">
                {TEMPLATES.map((t) => (
                  <div key={t.name} className="orchestrate-template-card">
                    <div className="orchestrate-template-card__icon">{t.icon}</div>
                    <div className="orchestrate-template-card__name">{t.name}</div>
                    <div className="orchestrate-template-card__desc">{t.description}</div>
                  </div>
                ))}
              </div>
            </div>
          ) : (
            <FlowCanvas />
          )}
        </div>
      </div>

      {selectedNodeId && (
        <div className="orchestrate-detail">
          <NodeDetailPanel />
        </div>
      )}
    </div>
  );
}
