import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  ReactFlow,
  Background,
  BackgroundVariant,
  Controls,
  Handle,
  Position,
  addEdge,
  type Node,
  type Edge,
  type Connection,
  type NodeProps,
  type OnConnect,
} from '@xyflow/react';
import '@xyflow/react/dist/style.css';
import {
  NODE_META,
  NODE_TYPE_ORDER,
  defaultParams,
  graphToYaml,
  yamlToGraph,
  type BuilderEdge,
  type BuilderGraph,
  type BuilderNode,
  type NodeType,
  type TransformOp,
} from './workflowSchema';

/**
 * C3 visual DAG builder — a React Flow canvas that composes the backend
 * node types by drag/connect and emits the `WorkflowDef` YAML the existing
 * `run_workflow` command already accepts. Mounted as a "可视化" tab next to the
 * raw YAML editor in OrchestrateView. Logic (serialize/parse/round-trip) lives
 * in `workflowBuilder.ts` and is unit-tested there; this component is the
 * interaction shell.
 *
 * Loop / selector / interrupt are supported (backend control-flow nodes in
 * graph.rs); the loop's sub-graph body is edited as inline JSON in the
 * inspector (a nested canvas is future work).
 */

type WorkflowNodeData = {
  node: BuilderNode;
  isStart: boolean;
  isEnd: boolean;
};

const GRID_X = 260;
const GRID_Y = 140;

function gridPosition(index: number): { x: number; y: number } {
  const perRow = 3;
  return {
    x: 40 + (index % perRow) * GRID_X,
    y: 40 + Math.floor(index / perRow) * GRID_Y,
  };
}

function uniqueId(existing: BuilderNode[], type: NodeType): string {
  let i = 1;
  let id = `${type}_${i}`;
  const taken = new Set(existing.map((n) => n.id));
  while (taken.has(id)) {
    i += 1;
    id = `${type}_${i}`;
  }
  return id;
}

function WorkflowNodeView({ data, selected }: NodeProps) {
  const d = data as WorkflowNodeData;
  const meta = NODE_META[d.node.type];
  const preview = previewParam(d.node);
  return (
    <div
      className={`wf-node wf-node--${d.node.type}${selected ? ' wf-node--selected' : ''}`}
      style={{ borderColor: meta.color }}
      title={meta.hint}
    >
      <Handle type="target" position={Position.Left} />
      <div className="wf-node-head" style={{ background: meta.color }}>
        <span className="wf-node-type">{meta.label}</span>
        {d.isStart && <span className="wf-node-flag" title="起点">起</span>}
        {d.isEnd && <span className="wf-node-flag" title="终点">终</span>}
      </div>
      <div className="wf-node-body">
        <span className="wf-node-id">{d.node.id}</span>
        {preview && <span className="wf-node-preview">{preview}</span>}
      </div>
      <Handle type="source" position={Position.Right} />
    </div>
  );
}

function previewParam(n: BuilderNode): string {
  switch (n.type) {
    case 'prompt': return n.text ?? '';
    case 'agent': return [n.agent, n.model].filter(Boolean).join(' · ');
    case 'gate': return n.gate ?? '';
    case 'branch': return n.condition ?? '';
    case 'human': return n.prompt ?? '';
    case 'transform': return opLabel(n.op);
    case 'selector': return Array.isArray(n.cases) ? `${n.cases.length} 分支` : '';
    case 'loop': return [n.over, n.count ? `${n.count}×` : null].filter(Boolean).join(' · ');
    case 'interrupt': return n.message ?? '';
    default: return '';
  }
}

function opLabel(op?: TransformOp): string {
  if (!op) return '';
  if ('extract' in op) return `extract ${op.extract}`;
  if ('wrap' in op) return 'wrap';
  if ('truncate' in op) return `truncate ${op.truncate}`;
  return '';
}

const nodeTypes = { workflow: WorkflowNodeView };

export interface WorkflowBuilderProps {
  /** Two-way bound YAML (the same `yaml` the run button consumes). */
  yaml: string;
  onYamlChange: (yaml: string) => void;
}

export function WorkflowBuilder({ yaml, onYamlChange }: WorkflowBuilderProps) {
  // Local canvas state, kept in sync with the bound YAML. We track positions
  // separately so drag doesn't fight the YAML round-trip (positions aren't in
  // the schema).
  const [graph, setGraph] = useState<BuilderGraph>(() => yamlToGraph(yaml));
  const [positions, setPositions] = useState<Record<string, { x: number; y: number }>>(() =>
    initialPositions(yamlToGraph(yaml)),
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // Avoid emitting YAML while we're ingesting an external change (re-import).
  const importingRef = useRef(false);
  // Mirrors of graph/positions for the yaml-driven effect below. That effect
  // intentionally lists only [yaml] as its dep, so the closure's graph/
  // positions would be stale; reading via refs acts on fresh state without
  // widening the dep array (which would re-run on every drag and fight the
  // round-trip).
  const graphRef = useRef(graph);
  graphRef.current = graph;
  const positionsRef = useRef(positions);
  positionsRef.current = positions;

  // Re-ingest when the bound YAML changes from the outside (e.g. user picks a
  // template in the YAML tab, or pastes YAML). Don't clobber positions for ids
  // we already know.
  useEffect(() => {
    importingRef.current = true;
    const prev = graphRef.current;
    const next = yamlToGraph(yaml);
    const updated = { ...positionsRef.current };
    let nextIdx = 0;
    for (const n of next.nodes) {
      if (!updated[n.id]) updated[n.id] = gridPosition(prev.nodes.length + nextIdx);
      nextIdx += 1;
    }
    // setGraph + setPositions as two SEPARATE, pure calls. The old code nested
    // setPositions INSIDE the setGraph updater — an impure updater that, under
    // React Strict Mode (double-invoke) or concurrent rendering, ran the inner
    // dispatch twice against a stale `prev` closure and mangled grid positions.
    // Stale positions for nodes that left the graph are intentionally kept
    // (harmless, avoids flicker if the node returns).
    setGraph(next);
    setPositions(updated);
    importingRef.current = false;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [yaml]);

  // Emit YAML whenever the canvas graph changes (but not mid-import).
  useEffect(() => {
    if (importingRef.current) return;
    onYamlChange(graphToYaml(graph));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [graph]);

  const rfNodes: Node<WorkflowNodeData>[] = useMemo(
    () =>
      graph.nodes.map((n) => ({
        id: n.id,
        type: 'workflow',
        position: positions[n.id] ?? { x: 40, y: 40 },
        data: {
          node: n,
          isStart: graph.startId === n.id,
          isEnd: graph.endId === n.id,
        },
        selected: n.id === selectedId,
      })),
    [graph, positions, selectedId],
  );

  const rfEdges: Edge[] = useMemo(
    () =>
      graph.edges.map((e) => ({
        id: e.id,
        source: e.source,
        target: e.target,
        // branch edges get a dashed style so the routing is visible.
        animated:
          graph.nodes.find((n) => n.id === e.source)?.type === 'branch',
      })),
    [graph],
  );

  const addNode = useCallback((type: NodeType) => {
    setGraph((prev) => {
      const id = uniqueId(prev.nodes, type);
      const node: BuilderNode = { id, type, ...defaultParams(type) };
      const next: BuilderGraph = {
        ...prev,
        nodes: [...prev.nodes, node],
        startId: prev.startId ?? id,
        endId: prev.nodes.length === 0 ? id : prev.endId,
      };
      setPositions((p) => ({ ...p, [id]: gridPosition(prev.nodes.length) }));
      setSelectedId(id);
      return next;
    });
  }, []);

  const onConnect: OnConnect = useCallback((conn: Connection) => {
    if (!conn.source || !conn.target) return;
    setGraph((prev) => {
      const id = `e${prev.edges.length}_${conn.source}_${conn.target}`;
      if (prev.edges.some((e) => e.source === conn.source && e.target === conn.target)) {
        return prev;
      }
      return {
        ...prev,
        edges: addEdge(
          { id, source: conn.source!, target: conn.target! },
          prev.edges as unknown as Edge[],
        ) as unknown as BuilderEdge[],
      };
    });
  }, []);

  const deleteSelected = useCallback(() => {
    if (!selectedId) return;
    setGraph((prev) => ({
      startId: prev.startId === selectedId ? null : prev.startId,
      endId: prev.endId === selectedId ? null : prev.endId,
      nodes: prev.nodes.filter((n) => n.id !== selectedId),
      edges: prev.edges.filter((e) => e.source !== selectedId && e.target !== selectedId),
    }));
    setSelectedId(null);
  }, [selectedId]);

  const updateNode = useCallback((id: string, patch: Partial<BuilderNode>) => {
    setGraph((prev) => ({
      ...prev,
      nodes: prev.nodes.map((n) => (n.id === id ? { ...n, ...patch } : n)),
    }));
  }, []);

  const setEndpoint = useCallback((id: string, which: 'start' | 'end') => {
    setGraph((prev) => ({ ...prev, [which === 'start' ? 'startId' : 'endId']: id }));
  }, []);

  const onNodesChange = useCallback((changes: { type: string; id?: string; position?: { x: number; y: number } }[]) => {
    // React Flow fires position-drag changes; persist them so re-render keeps
    // the dropped position. Selection changes drive selectedId.
    for (const c of changes) {
      if (c.type === 'position' && c.id && c.position) {
        setPositions((p) => ({ ...p, [c.id!]: c.position! }));
      }
      if (c.type === 'select' && c.id !== undefined) {
        setSelectedId(c.id === selectedId ? selectedId : c.id ?? null);
      }
    }
  }, [selectedId]);

  const selected = graph.nodes.find((n) => n.id === selectedId) ?? null;

  return (
    <div className="wf-builder">
      <aside className="wf-palette">
        <h4>节点</h4>
        {NODE_TYPE_ORDER.map((t) => (
          <button
            key={t}
            type="button"
            className="wf-palette-item"
            onClick={() => addNode(t)}
            title={NODE_META[t].hint}
          >
            <span className="wf-palette-dot" style={{ background: NODE_META[t].color }} />
            {NODE_META[t].label}
          </button>
        ))}
        <hr />
        <button type="button" className="wf-palette-item" onClick={deleteSelected} disabled={!selectedId}>
          删除选中
        </button>
      </aside>

      <div className="wf-canvas">
        <ReactFlow
          nodes={rfNodes}
          edges={rfEdges}
          nodeTypes={nodeTypes}
          onConnect={onConnect}
          onNodesChange={onNodesChange as never}
          onNodeClick={(_, n) => setSelectedId(n.id)}
          fitView
          proOptions={{ hideAttribution: true }}
        >
          <Background variant={BackgroundVariant.Dots} gap={16} />
          <Controls showInteractive={false} />
        </ReactFlow>
      </div>

      <aside className="wf-inspector">
        <h4>属性</h4>
        {!selected && <p className="muted">点选一个节点编辑参数 / 设为起点终点</p>}
        {selected && (
          <NodeInspector
            node={selected}
            isStart={graph.startId === selected.id}
            isEnd={graph.endId === selected.id}
            onChange={(patch) => updateNode(selected.id, patch)}
            onSetEndpoint={(which) => setEndpoint(selected.id, which)}
          />
        )}
      </aside>
    </div>
  );
}

function NodeInspector({
  node,
  isStart,
  isEnd,
  onChange,
  onSetEndpoint,
}: {
  node: BuilderNode;
  isStart: boolean;
  isEnd: boolean;
  onChange: (patch: Partial<BuilderNode>) => void;
  onSetEndpoint: (which: 'start' | 'end') => void;
}) {
  const meta = NODE_META[node.type];
  return (
    <div className="wf-inspector-body">
      <div className="wf-inspector-row">
        <label>ID</label>
        <input value={node.id} onChange={(e) => onChange({ id: e.target.value })} />
      </div>
      <div className="wf-inspector-endpoints">
        <button type="button" className={`btn ${isStart ? 'btn-primary' : ''}`} onClick={() => onSetEndpoint('start')}>
          {isStart ? '✓ 起点' : '设为起点'}
        </button>
        <button type="button" className={`btn ${isEnd ? 'btn-primary' : ''}`} onClick={() => onSetEndpoint('end')}>
          {isEnd ? '✓ 终点' : '设为终点'}
        </button>
      </div>
      {node.type === 'transform' ? (
        <TransformEditor op={node.op} onChange={(op) => onChange({ op })} />
      ) : (
        meta.fields.map((f) => (
          <FieldEditor key={f.key} field={f} node={node} onChange={onChange} />
        ))
      )}
    </div>
  );
}

function FieldEditor({
  field,
  node,
  onChange,
}: {
  field: import('./workflowSchema').FieldDef;
  node: BuilderNode;
  onChange: (patch: Partial<BuilderNode>) => void;
}) {
  const value = (node as unknown as Record<string, unknown>)[field.key] ?? '';
  const set = (v: unknown) => onChange({ [field.key]: v } as Partial<BuilderNode>);
  return (
    <div className="wf-inspector-row">
      <label>
        {field.label}
        {field.required && <span className="wf-required">*</span>}
      </label>
      {field.input === 'textarea' && (
        <textarea value={String(value)} onChange={(e) => set(e.target.value)} rows={3} placeholder={field.placeholder} />
      )}
      {field.input === 'text' && (
        <input value={String(value)} onChange={(e) => set(e.target.value)} placeholder={field.placeholder} />
      )}
      {field.input === 'number' && (
        <input
          type="number"
          value={typeof value === 'number' ? value : ''}
          onChange={(e) => set(e.target.value === '' ? undefined : Number(e.target.value))}
        />
      )}
      {field.input === 'select' && field.options && (
        <select value={String(value)} onChange={(e) => set(e.target.value)}>
          {field.options.map((o) => (
            <option key={o} value={o}>
              {o}
            </option>
          ))}
        </select>
      )}
      {field.input === 'json' && (
        <JsonField value={value} onChange={set} />
      )}
    </div>
  );
}

function JsonField({ value, onChange }: { value: unknown; onChange: (v: unknown) => void }) {
  const [text, setText] = useState(() => (value === undefined ? '' : JSON.stringify(value)));
  const [err, setErr] = useState<string | null>(null);
  return (
    <>
      <textarea
        value={text}
        onChange={(e) => {
          setText(e.target.value);
          if (e.target.value.trim() === '') {
            setErr(null);
            onChange(undefined);
            return;
          }
          try {
            onChange(JSON.parse(e.target.value));
            setErr(null);
          } catch (e2) {
            setErr(String(e2));
          }
        }}
        rows={3}
        placeholder='{}'
      />
      {err && <span className="wf-error">JSON: {err}</span>}
    </>
  );
}

function TransformEditor({ op, onChange }: { op?: TransformOp; onChange: (op: TransformOp) => void }) {
  const kind: 'extract' | 'wrap' | 'truncate' = op
    ? 'extract' in op
      ? 'extract'
      : 'wrap' in op
        ? 'wrap'
        : 'truncate'
    : 'extract';
  return (
    <div className="wf-inspector-row">
      <label>变换操作</label>
      <select
        value={kind}
        onChange={(e) => {
          const k = e.target.value as 'extract' | 'wrap' | 'truncate';
          if (k === 'extract') onChange({ extract: 'output' });
          if (k === 'wrap') onChange({ wrap: { prefix: '', suffix: '' } });
          if (k === 'truncate') onChange({ truncate: 100 });
        }}
      >
        <option value="extract">extract（取字段）</option>
        <option value="wrap">wrap（前后缀包装）</option>
        <option value="truncate">truncate（截断 N 字符）</option>
      </select>
      {kind === 'extract' && op && 'extract' in op && (
        <input
          value={op.extract}
          onChange={(e) => onChange({ extract: e.target.value })}
          placeholder="output.summary"
        />
      )}
      {kind === 'wrap' && op && 'wrap' in op && (
        <>
          <input
            value={op.wrap.prefix}
            onChange={(e) => onChange({ wrap: { ...op.wrap, prefix: e.target.value } })}
            placeholder="前缀"
          />
          <input
            value={op.wrap.suffix}
            onChange={(e) => onChange({ wrap: { ...op.wrap, suffix: e.target.value } })}
            placeholder="后缀"
          />
        </>
      )}
      {kind === 'truncate' && op && 'truncate' in op && (
        <input
          type="number"
          value={op.truncate}
          onChange={(e) => onChange({ truncate: Number(e.target.value) })}
        />
      )}
    </div>
  );
}

function initialPositions(graph: BuilderGraph): Record<string, { x: number; y: number }> {
  const map: Record<string, { x: number; y: number }> = {};
  graph.nodes.forEach((n, i) => {
    map[n.id] = gridPosition(i);
  });
  return map;
}
