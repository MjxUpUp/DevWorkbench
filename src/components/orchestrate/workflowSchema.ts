import yaml from 'js-yaml';

/**
 * C3 visual DAG builder — serialization bridge between the React Flow canvas
 * and the backend `WorkflowDef` YAML schema (kernel-compose `yaml.rs:32-50`).
 *
 * The backend graph engine is fully reachable (`run_workflow(yaml_content, …)`);
 * this module's only job is to round-trip the canvas state ↔ the YAML text the
 * backend already accepts, so a non-technical user can compose a 2–5 node graph
 * by drag/connect instead of hand-writing YAML.
 *
 * Node taxonomy mirrors the backend (graph.rs:27-43): 7 base + branch + the
 * three coze/dify control-flow nodes. `loop` repeats an inline sub-graph
 * `body` (the repetition lives inside the node, never in the edges, so Kahn's
 * cycle check never sees it); `selector` emits the first-match label for
 * branch edges to route on; `interrupt` halts the run.
 */

export type NodeType =
  | 'prompt'
  | 'agent'
  | 'gate'
  | 'parallel'
  | 'merge'
  | 'human'
  | 'transform'
  | 'branch'
  | 'loop'
  | 'selector'
  | 'interrupt';

/** `TransformOp` mirror (graph.rs:144-153) — serde newtype/struct enum → nested. */
export type TransformOp =
  | { extract: string }
  | { wrap: { prefix: string; suffix: string } }
  | { truncate: number };

export interface BuilderNode {
  id: string;
  type: NodeType;
  // Flat params — presence is type-dependent (see NODE_META). Optional fields
  // are omitted from the emitted YAML when empty so the dump stays clean.
  text?: string;
  agent?: string;
  model?: string;
  mode?: string;
  prompt?: string;
  resume_from?: string;
  knowledge?: string[];
  skills?: string[];
  mcp_tools?: string[];
  gate?: string;
  config?: unknown;
  branches?: number;
  strategy?: string;
  op?: TransformOp;
  condition?: string;
  vars?: Record<string, unknown>;
  // selector: ordered first-match cases; default label when none match.
  cases?: Array<{ when: string; label: string }>;
  default?: string;
  // loop: iterate an input array (over) or a fixed count; body is an inline
  // sub-graph WorkflowDef the runner executes once per element/iteration.
  over?: string;
  count?: number;
  max_iterations?: number;
  body?: unknown;
  // interrupt: halt reason (+ optional gate condition).
  message?: string;
}

export interface BuilderEdge {
  id: string;
  source: string;
  target: string;
  /** Branch-edge guard expression (only meaningful when source is a `branch`). */
  when?: string;
}

export interface BuilderGraph {
  startId: string | null;
  endId: string | null;
  nodes: BuilderNode[];
  edges: BuilderEdge[];
}

export interface FieldDef {
  key: string;
  label: string;
  input: 'text' | 'textarea' | 'number' | 'select' | 'json';
  options?: string[];
  required?: boolean;
  placeholder?: string;
}

export interface NodeMeta {
  label: string;
  color: string;
  /** Inspector fields. `transform` is special-cased (op sub-editor) so its
   *  field list is empty here. */
  fields: FieldDef[];
  hint: string;
}

export const NODE_META: Record<NodeType, NodeMeta> = {
  prompt: {
    label: 'Prompt',
    color: '#6366f1',
    hint: '固定/模板提示词,作为图的起点播种任务',
    fields: [{ key: 'text', label: '文本', input: 'textarea', required: true }],
  },
  agent: {
    label: 'Agent',
    color: '#3b82f6',
    hint: '运行一个 agent(外部 CLI 或透明 ReactAgent)',
    fields: [
      { key: 'agent', label: 'Agent', input: 'select', required: true, placeholder: 'claude_code' },
      { key: 'model', label: '模型', input: 'select', placeholder: 'sonnet' },
      { key: 'mode', label: '权限级别', input: 'select', options: ['default', 'auto-edit', 'plan', 'dry-run', 'skip-permissions'] },
      { key: 'prompt', label: '提示词', input: 'textarea', placeholder: '留空则用上游输入' },
      { key: 'knowledge', label: '知识库 ID', input: 'json', placeholder: '[]' },
      { key: 'skills', label: 'Skills', input: 'json', placeholder: '[]' },
      { key: 'mcp_tools', label: 'MCP 工具', input: 'json', placeholder: '[]' },
      { key: 'resume_from', label: '恢复自', input: 'text' },
    ],
  },
  gate: {
    label: 'Gate',
    color: '#f59e0b',
    hint: '质量门禁(forge / honesty / compile / test …)',
    fields: [
      { key: 'gate', label: '门禁', input: 'select', required: true, options: ['forge', 'honesty', 'compile', 'test'] },
      { key: 'config', label: '配置 (JSON)', input: 'json' },
    ],
  },
  parallel: {
    label: 'Parallel',
    color: '#8b5cf6',
    hint: '扇出:并发激活所有后继分支',
    fields: [{ key: 'branches', label: '分支数', input: 'number' }],
  },
  merge: {
    label: 'Merge',
    color: '#14b8a6',
    hint: '扇入:等待所有前驱并合并输出',
    fields: [
      { key: 'strategy', label: '策略', input: 'select', options: ['concat', 'last_wins', 'collect'] },
    ],
  },
  human: {
    label: 'Human',
    color: '#ec4899',
    hint: '暂停等待人工审批后继续',
    fields: [{ key: 'prompt', label: '审批提示', input: 'textarea' }],
  },
  transform: {
    label: 'Transform',
    color: '#0ea5e9',
    hint: '纯数据变换(取字段/包装/截断),不改控制流',
    fields: [], // op is rendered by a dedicated sub-editor in the inspector.
  },
  branch: {
    label: 'Branch',
    color: '#ef4444',
    hint: '条件路由,按 condition 选择激活的后继',
    fields: [
      { key: 'condition', label: '条件', input: 'text', required: true, placeholder: 'key==value 或 contains:子串' },
    ],
  },
  selector: {
    label: 'Selector',
    color: '#a855f7',
    hint: '条件选择:首个 when 命中的分支胜出,输出 label 供下游 branch 边路由',
    fields: [
      { key: 'cases', label: '分支 (JSON)', input: 'json', required: true, placeholder: '[{"when":"contains:a","label":"a_path"}]' },
      { key: 'default', label: '默认 label', input: 'text' },
    ],
  },
  loop: {
    label: 'Loop',
    color: '#06b6d4',
    hint: '循环:对数组(over)或固定次数(count)迭代子图 body,按 strategy 合并',
    fields: [
      { key: 'over', label: '迭代数组路径', input: 'text', placeholder: 'items' },
      { key: 'count', label: '固定次数', input: 'number', placeholder: 'over 缺省时用' },
      { key: 'max_iterations', label: '最大迭代', input: 'number', placeholder: '默认 1000' },
      { key: 'strategy', label: '合并策略', input: 'select', options: ['concat', 'last_wins', 'collect'] },
      { key: 'body', label: '子图 (JSON)', input: 'json', placeholder: '内联 WorkflowDef' },
    ],
  },
  interrupt: {
    label: 'Interrupt',
    color: '#dc2626',
    hint: '终止:用户意图停止整个图(非失败);带 condition 时仅在满足时终止',
    fields: [
      { key: 'message', label: '终止原因', input: 'textarea' },
      { key: 'condition', label: '条件 (可选)', input: 'text', placeholder: '留空则无条件终止' },
    ],
  },
};

export const NODE_TYPE_ORDER: NodeType[] = [
  'prompt', 'agent', 'gate', 'parallel', 'merge', 'human', 'transform', 'branch',
  'selector', 'loop', 'interrupt',
];

/** Sensible defaults for a freshly-added node of each type. */
export function defaultParams(type: NodeType): Partial<BuilderNode> {
  switch (type) {
    case 'prompt': return { text: '' };
    case 'agent': return { agent: 'claude_code' };
    case 'gate': return { gate: 'forge' };
    case 'parallel': return { branches: 2 };
    case 'merge': return { strategy: 'concat' };
    case 'human': return { prompt: '' };
    case 'transform': return { op: { extract: 'output' } };
    case 'branch': return { condition: '' };
    case 'selector': return { cases: [{ when: 'contains:a', label: 'a_path' }] };
    case 'loop': return {
      count: 3,
      strategy: 'collect',
      body: { start: 'iter', end: 'iter', nodes: { iter: { type: 'transform', op: { extract: 'output' } } }, edges: [] },
    };
    case 'interrupt': return { message: '' };
  }
}

/** Serialize the canvas graph into the backend `WorkflowDef` YAML. */
export function graphToYaml(g: BuilderGraph): string {
  const nodes: Record<string, Record<string, unknown>> = {};
  for (const n of g.nodes) {
    const obj: Record<string, unknown> = { type: n.type };
    for (const f of NODE_META[n.type].fields) {
      const v = (n as unknown as Record<string, unknown>)[f.key];
      if (v === undefined || v === null) continue;
      if (typeof v === 'string' && v === '') continue;
      obj[f.key] = f.input === 'number' ? Number(v) : v;
    }
    // transform: emit the nested op object verbatim.
    if (n.type === 'transform' && n.op) obj.op = n.op;
    nodes[n.id] = obj;
  }

  const startId = g.startId ?? g.nodes[0]?.id ?? '';
  const endId = g.endId ?? g.nodes[g.nodes.length - 1]?.id ?? '';

  const def: Record<string, unknown> = {
    start: startId,
    end: endId,
    nodes,
    edges: g.edges.map((e) => {
      const src = g.nodes.find((n) => n.id === e.source);
      const edge: Record<string, unknown> = { from: e.source, to: e.target };
      // The backend derives routing from the source node type; mark branch
      // edges explicitly so the runner knows to evaluate `when`.
      if (src?.type === 'branch') edge.kind = 'branch';
      if (e.when) edge.when = e.when;
      return edge;
    }),
  };
  return yaml.dump(def, { lineWidth: 120, noRefs: true });
}

/** Parse a `WorkflowDef` YAML string back into the canvas shape. Tolerant: a
 *  malformed/empty doc yields an empty graph rather than throwing, so the
 *  canvas never crashes on bad hand-edited YAML. */
export function yamlToGraph(raw: string): BuilderGraph {
  const empty: BuilderGraph = { startId: null, endId: null, nodes: [], edges: [] };
  if (!raw.trim()) return empty;
  let doc: unknown;
  try {
    doc = yaml.load(raw);
  } catch {
    return empty;
  }
  if (!doc || typeof doc !== 'object') return empty;
  const d = doc as {
    start?: string;
    end?: string;
    nodes?: Record<string, Record<string, unknown>>;
    edges?: Array<{ from: string; to: string; when?: string }>;
  };

  const nodes: BuilderNode[] = [];
  if (d.nodes) {
    for (const [id, body] of Object.entries(d.nodes)) {
      if (!body || typeof body !== 'object') continue;
      const type = body.type as NodeType | undefined;
      if (!type || !(type in NODE_META)) continue;
      const n: BuilderNode = { id, type };
      for (const f of NODE_META[type].fields) {
        if (body[f.key] !== undefined) (n as unknown as Record<string, unknown>)[f.key] = body[f.key];
      }
      if (type === 'transform' && body.op) n.op = body.op as TransformOp;
      nodes.push(n);
    }
  }

  const edges: BuilderEdge[] = (d.edges ?? []).map((e, i) => ({
    id: `e${i}_${e.from}_${e.to}`,
    source: e.from,
    target: e.to,
    when: e.when,
  }));

  return {
    startId: d.start ?? null,
    endId: d.end ?? null,
    nodes,
    edges,
  };
}

/** Round-trip helper used by tests + the "apply then re-import" button. */
export function roundTrip(g: BuilderGraph): BuilderGraph {
  return yamlToGraph(graphToYaml(g));
}
