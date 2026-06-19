import { describe, it, expect } from 'vitest';
import yaml from 'js-yaml';
import {
  graphToYaml,
  yamlToGraph,
  roundTrip,
  defaultParams,
  NODE_META,
  type BuilderGraph,
} from '../workflowSchema';

const SAMPLE_YAML = `
start: prompt_1
end: gate_1
nodes:
  prompt_1:
    type: prompt
    text: refactor auth
  agent_1:
    type: agent
    agent: claude_code
    model: sonnet
  gate_1:
    type: gate
    gate: forge
edges:
  - from: prompt_1
    to: agent_1
  - from: agent_1
    to: gate_1
`;

describe('workflowSchema serialize/parse', () => {
  it('yamlToGraph reads the canonical sample into the canvas shape', () => {
    const g = yamlToGraph(SAMPLE_YAML);
    expect(g.startId).toBe('prompt_1');
    expect(g.endId).toBe('gate_1');
    expect(g.nodes.map((n) => n.id)).toEqual(['prompt_1', 'agent_1', 'gate_1']);
    const agent = g.nodes.find((n) => n.id === 'agent_1')!;
    expect(agent.type).toBe('agent');
    expect(agent.agent).toBe('claude_code');
    expect(agent.model).toBe('sonnet');
    expect(g.edges.map((e) => [e.source, e.target])).toEqual([
      ['prompt_1', 'agent_1'],
      ['agent_1', 'gate_1'],
    ]);
  });

  it('graphToYaml emits the WorkflowDef schema the backend parses', () => {
    const g: BuilderGraph = {
      startId: 'p1',
      endId: 'g1',
      nodes: [
        { id: 'p1', type: 'prompt', text: 'do work' },
        { id: 'g1', type: 'gate', gate: 'forge' },
      ],
      edges: [{ id: 'e1', source: 'p1', target: 'g1' }],
    };
    const out = graphToYaml(g);
    const reparsed = yaml.load(out) as {
      start: string; end: string;
      nodes: Record<string, { type: string; [k: string]: unknown }>;
      edges: Array<{ from: string; to: string }>;
    };
    expect(reparsed.start).toBe('p1');
    expect(reparsed.end).toBe('g1');
    expect(reparsed.nodes.p1).toEqual({ type: 'prompt', text: 'do work' });
    expect(reparsed.nodes.g1).toEqual({ type: 'gate', gate: 'forge' });
    expect(reparsed.edges).toEqual([{ from: 'p1', to: 'g1' }]);
  });

  it('round-trip preserves nodes, edges, start, end', () => {
    const g: BuilderGraph = {
      startId: 'a',
      endId: 'c',
      nodes: [
        { id: 'a', type: 'agent', agent: 'codex', model: 'opus' },
        { id: 'b', type: 'human', prompt: 'ok?' },
        { id: 'c', type: 'merge', strategy: 'collect' },
      ],
      edges: [
        { id: 'e1', source: 'a', target: 'b' },
        { id: 'e2', source: 'b', target: 'c' },
      ],
    };
    const back = roundTrip(g);
    expect(back.startId).toBe('a');
    expect(back.endId).toBe('c');
    expect(back.nodes.map((n) => n.id)).toEqual(['a', 'b', 'c']);
    expect(back.nodes.find((n) => n.id === 'a')?.model).toBe('opus');
    expect(back.nodes.find((n) => n.id === 'c')?.strategy).toBe('collect');
    expect(back.edges.map((e) => [e.source, e.target])).toEqual([
      ['a', 'b'],
      ['b', 'c'],
    ]);
  });

  it('marks edges leaving a branch node with kind: branch', () => {
    const g: BuilderGraph = {
      startId: 'br',
      endId: 'end',
      nodes: [
        { id: 'br', type: 'branch', condition: 'status==ok' },
        { id: 'ok', type: 'prompt', text: 'yes' },
        { id: 'end', type: 'merge', strategy: 'concat' },
      ],
      edges: [
        { id: 'e1', source: 'br', target: 'ok', when: 'status==ok' },
        { id: 'e2', source: 'ok', target: 'end' },
      ],
    };
    const out = graphToYaml(g);
    const reparsed = yaml.load(out) as { edges: Array<{ from: string; to: string; kind?: string; when?: string }> };
    expect(reparsed.edges[0]).toMatchObject({ from: 'br', to: 'ok', kind: 'branch', when: 'status==ok' });
    // Normal edge has no kind field.
    expect(reparsed.edges[1].kind).toBeUndefined();
  });

  it('round-trips the transform op (extract/wrap/truncate)', () => {
    const g: BuilderGraph = {
      startId: 't',
      endId: 't',
      nodes: [{ id: 't', type: 'transform', op: { wrap: { prefix: '<<', suffix: '>>' } } }],
      edges: [],
    };
    const back = roundTrip(g);
    expect(back.nodes[0].op).toEqual({ wrap: { prefix: '<<', suffix: '>>' } });
  });

  it('omits empty optional fields so the YAML stays clean', () => {
    const g: BuilderGraph = {
      startId: 'a',
      endId: 'a',
      nodes: [{ id: 'a', type: 'agent', agent: 'claude_code', model: '', prompt: '' }],
      edges: [],
    };
    const out = graphToYaml(g);
    const reparsed = yaml.load(out) as { nodes: { a: Record<string, unknown> } };
    expect(reparsed.nodes.a).toEqual({ type: 'agent', agent: 'claude_code' });
    expect(reparsed.nodes.a.model).toBeUndefined();
  });

  it('tolerates malformed/empty YAML without throwing', () => {
    expect(yamlToGraph('')).toEqual({ startId: null, endId: null, nodes: [], edges: [] });
    expect(yamlToGraph('::: not yaml :::\n  - [unclosed').nodes).toEqual([]);
    // Unknown node type is dropped, not crashed on (loop is not a real type).
    const g = yamlToGraph('start: x\nend: x\nnodes:\n  x:\n    type: loop\n');
    expect(g.nodes).toEqual([]);
  });

  it('falls back to first/last node id when start/end are unset', () => {
    const g: BuilderGraph = {
      startId: null,
      endId: null,
      nodes: [
        { id: 'first', type: 'prompt', text: 'a' },
        { id: 'last', type: 'gate', gate: 'forge' },
      ],
      edges: [],
    };
    const out = graphToYaml(g);
    const reparsed = yaml.load(out) as { start: string; end: string };
    expect(reparsed.start).toBe('first');
    expect(reparsed.end).toBe('last');
  });

  it('defaultParams seeds each type with a usable baseline (no loop/selector/interrupt)', () => {
    expect(defaultParams('agent').agent).toBe('claude_code');
    expect(defaultParams('gate').gate).toBe('forge');
    expect(defaultParams('transform').op).toEqual({ extract: 'output' });
    for (const t of ['prompt', 'agent', 'gate', 'parallel', 'merge', 'human', 'transform', 'branch'] as const) {
      expect(NODE_META[t]).toBeTruthy();
    }
    // The builder must not offer node types the backend can't run.
    expect((NODE_META as Record<string, unknown>).loop).toBeUndefined();
    expect((NODE_META as Record<string, unknown>).selector).toBeUndefined();
  });
});
