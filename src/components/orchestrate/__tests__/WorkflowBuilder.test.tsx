import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

// React Flow needs ResizeObserver + canvas measuring jsdom lacks. Stub the
// canvas shell; the real interaction logic lives in workflowSchema.ts (tested
// separately). We keep onConnect wiring so addNode→emit can be exercised.
vi.mock('@xyflow/react', () => {
  // JSX auto-runtime is available in the factory — no React import needed.
  const ReactFlow = (props: any) => (
    <div data-testid="react-flow">
      {/* surface nodes so tests can assert what the canvas was handed */}
      {props.nodes?.map((n: any) => (
        <div key={n.id} data-testid={`rf-node-${n.id}`} onClick={() => props.onNodeClick?.({}, n)}>
          {n.data.node.type}:{n.id}
          {n.data.isStart ? ' [起]' : ''}
          {n.data.isEnd ? ' [终]' : ''}
        </div>
      ))}
      {props.edges?.map((e: any) => (
        <div key={e.id} data-testid={`rf-edge-${e.id}`}>{e.source} -&gt; {e.target}</div>
      ))}
    </div>
  );
  return {
    ReactFlow,
    Background: () => null,
    BackgroundVariant: { Dots: 'dots' },
    Controls: () => null,
    Handle: () => null,
    Position: { Left: 'left', Right: 'right' },
    addEdge: (edge: any, edges: any[]) => [...edges, edge],
  };
});

import { WorkflowBuilder } from '../WorkflowBuilder';
import yaml from 'js-yaml';

describe('WorkflowBuilder', () => {
  it('renders the 8-type palette (no loop/selector/interrupt)', () => {
    const { container } = render(<WorkflowBuilder yaml="" onYamlChange={() => {}} />);
    const palette = container.querySelector('.wf-palette');
    expect(palette?.textContent).toContain('Prompt');
    expect(palette?.textContent).toContain('Agent');
    expect(palette?.textContent).toContain('Gate');
    expect(palette?.textContent).toContain('Parallel');
    expect(palette?.textContent).toContain('Merge');
    expect(palette?.textContent).toContain('Human');
    expect(palette?.textContent).toContain('Transform');
    expect(palette?.textContent).toContain('Branch');
    expect(palette?.textContent).not.toContain('Loop');
  });

  it('clicking a palette item adds a node and emits YAML via onYamlChange', async () => {
    const onChange = vi.fn();
    render(<WorkflowBuilder yaml="" onYamlChange={onChange} />);
    fireEvent.click(screen.getByText('Agent'));
    await waitFor(() => {
      // The newest emitted YAML must contain an agent node with the default id.
      const last = onChange.mock.calls[onChange.mock.calls.length - 1]?.[0] as string;
      const parsed = yaml.load(last) as {
        start: string; end: string;
        nodes: Record<string, { type: string; agent: string }>;
      };
      expect(parsed.nodes.agent_1).toEqual({ type: 'agent', agent: 'claude_code' });
      // First node added becomes both start and end.
      expect(parsed.start).toBe('agent_1');
      expect(parsed.end).toBe('agent_1');
    });
  });

  it('ingests an existing YAML and renders its nodes on the canvas', async () => {
    const existing = `
start: p1
end: g1
nodes:
  p1: { type: prompt, text: hi }
  g1: { type: gate, gate: forge }
edges:
  - { from: p1, to: g1 }
`;
    render(<WorkflowBuilder yaml={existing} onYamlChange={() => {}} />);
    expect(await screen.findByTestId('rf-node-p1')).toBeTruthy();
    expect(screen.getByTestId('rf-node-g1')).toBeTruthy();
    expect(screen.getByTestId('rf-edge-e0_p1_g1')).toBeTruthy();
  });

  it('selecting a node opens the inspector with its type fields', async () => {
    const existing = `
start: p1
end: p1
nodes:
  p1: { type: prompt, text: hello }
`;
    render(<WorkflowBuilder yaml={existing} onYamlChange={() => {}} />);
    fireEvent.click(await screen.findByTestId('rf-node-p1'));
    // Prompt node inspector shows the text field label + endpoint controls
    // (button text is "✓ 起点"/"设为起点" depending on state; both contain 起点).
    const inspector = document.querySelector('.wf-inspector');
    expect(inspector?.textContent).toContain('文本');
    expect(inspector?.textContent).toContain('起点');
    expect(inspector?.textContent).toContain('终点');
  });
});
