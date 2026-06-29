import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { useNavigationStore } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';
import { useProvidersStore } from '../../stores/providersStore';
import { Button } from '../ui/Button/Button';
import {
  parseNodeIds,
  useOrchestrateStore,
  type NodeState,
} from '../../stores/orchestrateStore';
// BlocksView used in event log for running node streams (added back when needed)
// import { BlocksView } from '../chat/BlocksView';
import { WorkflowBuilder } from './WorkflowBuilder';
import type { BuilderNode } from './workflowSchema';
import type { ChatStreamEvent, Workflow, WorkflowProgressPayload, WorkflowRunResult, WorkflowTemplate } from '../../types';

const STATUS_COLOR: Record<NodeState['status'], string> = {
  pending: 'var(--gate-skip)',
  running: 'var(--status-running)',
  done: 'var(--gate-pass)',
  failed: 'var(--gate-fail)',
  skipped: 'var(--gate-skip)',
  waiting_approval: 'var(--gate-warn)',
};

const STATUS_LABEL: Record<NodeState['status'], string> = {
  pending: '待执行',
  running: '执行中',
  done: '完成',
  failed: '失败',
  skipped: '已跳过',
  waiting_approval: '等待审批',
};

type SidebarTab = 'inspector' | 'palette' | 'history';

export function OrchestrateView() {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const yaml = useOrchestrateStore((s) => s.yaml);
  const setYaml = useOrchestrateStore((s) => s.setYaml);
  const nodes = useOrchestrateStore((s) => s.nodes);
  const runId = useOrchestrateStore((s) => s.runId);
  const output = useOrchestrateStore((s) => s.output);
  const error = useOrchestrateStore((s) => s.error);
  const pendingApproval = useOrchestrateStore((s) => s.pendingApproval);
  const applyEvent = useOrchestrateStore((s) => s.applyEvent);
  const approve = useOrchestrateStore((s) => s.approve);
  const startRun = useOrchestrateStore((s) => s.startRun);
  const reset = useOrchestrateStore((s) => s.reset);
  const currentWorkflowId = useOrchestrateStore((s) => s.currentWorkflowId);
  const setCurrentWorkflowId = useOrchestrateStore((s) => s.setCurrentWorkflowId);
  const savedWorkflows = useOrchestrateStore((s) => s.savedWorkflows);
  const setSavedWorkflows = useOrchestrateStore((s) => s.setSavedWorkflows);
  // Agent/model dropdowns in the WorkflowBuilder node inspector read these
  // global stores. ChatView/Settings load them; load here too so the orchestrate
  // page works standalone (entering it directly left both empty → no options).
  const refreshAgents = useAgentStore((s) => s.refreshAgents);
  const loadProviders = useProvidersStore((s) => s.loadProviders);

  const [eventLog, setEventLog] = useState<string[]>([]);
  const [templates, setTemplates] = useState<WorkflowTemplate[]>([]);
  const [editorMode, setEditorMode] = useState<'visual' | 'yaml'>('visual');
  const [sidebarTab, setSidebarTab] = useState<SidebarTab>('inspector');
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [logCollapsed, setLogCollapsed] = useState(false);
  const [wfSelectedId, setWfSelectedId] = useState<string | null>(null);
  const [wfSelectedNode, setWfSelectedNode] = useState<BuilderNode | null>(null);
  // Save dialog state — 保存/另存为 共用：saveMode 决定确认时走 update 还是 create
  const [saveDialogOpen, setSaveDialogOpen] = useState(false);
  const [saveMode, setSaveMode] = useState<'save' | 'saveAs'>('save');
  const [saveName, setSaveName] = useState('');
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  useEffect(() => {
    invoke<WorkflowTemplate[]>('list_workflow_templates')
      .then(setTemplates)
      .catch(() => setTemplates([]));
    // WorkflowBuilder's agent/model <select>s read useAgentStore.agents +
    // useProvidersStore.config. ChatView/Settings load those; without loading
    // here, entering orchestrate directly left both empty → dropdowns had no
    // options. Sync with the unified agent + model management.
    void refreshAgents();
    void loadProviders();
  }, [refreshAgents, loadProviders]);

  // Single source of truth: running === runId !== null. The store flips runId
  // to non-null on startRun and back to null on graph_done / graph_failed /
  // reset (see orchestrateStore.applyEvent + reset). Deriving (instead of a
  // parallel useState that mirrors the same transitions) avoids the deadlock
  // where the boolean never resets if the backend panics / disconnects and the
  // terminal event never arrives — runId is what actually reflects run lifecycle.
  const running = runId !== null;

  useEffect(() => {
    let unlisten: (() => void) | null = null;
    let cancelled = false;
    (async () => {
      const fn = await listen<WorkflowProgressPayload>('workflow:progress', (e) => {
        const { event } = e.payload;
        applyEvent(event);
        setEventLog((prev) => [...prev.slice(-50), formatEvent(event)]);
      });
      if (cancelled) fn();
      else unlisten = fn;
    })();
    return () => { cancelled = true; if (unlisten) unlisten(); };
  }, [applyEvent]);

  const nodeIds = parseNodeIds(yaml);
  const runningNode = Object.entries(nodes).find(([, s]) => s.status === 'running')?.[0];

  const handleRun = async () => {
    setEventLog([]);
    reset();
    try {
      const result = await invoke<WorkflowRunResult>('run_workflow', {
        yamlContent: yaml,
        input: { task: 'orchestrate run' },
        workingDir: activeProject?.path ?? null,
      });
      startRun(result.run_id);
    } catch (e) {
      setEventLog((prev) => [...prev, `[error] ${String(e)}`]);
    }
  };

  // 拉取已保存的 workflow 列表（历史 tab 用）。切换到历史 tab 时触发。
  const refreshSaved = async () => {
    try {
      const list = await invoke<Workflow[]>('list_workflows');
      setSavedWorkflows(Array.isArray(list) ? list : []);
    } catch (e) {
      setSavedWorkflows([]);
      console.error('list_workflows failed', e);
    }
  };

  // "保存"：已存（currentWorkflowId 非空）→ 直接 update_workflow 覆盖；
  // 否则打开对话框让用户命名后 create_workflow。
  const handleSave = () => {
    if (currentWorkflowId) {
      const current = savedWorkflows.find((w) => w.id === currentWorkflowId);
      void doSave('save', current?.name ?? 'workflow');
    } else {
      setSaveMode('save');
      setSaveName('');
      setSaveError(null);
      setSaveDialogOpen(true);
    }
  };

  // "另存为"：始终新建（即便当前已存），打开对话框。
  const handleSaveAs = () => {
    setSaveMode('saveAs');
    setSaveName('');
    setSaveError(null);
    setSaveDialogOpen(true);
  };

  // 实际落库：save 且 currentWorkflowId 存在 → update；否则 create（saveAs 或首次 save）。
  const doSave = async (mode: 'save' | 'saveAs', name: string) => {
    const trimmed = name.trim();
    if (!trimmed) {
      setSaveError('请输入工作流名称');
      return;
    }
    setSaving(true);
    setSaveError(null);
    try {
      const isUpdate = mode === 'save' && currentWorkflowId;
      if (isUpdate) {
        const wf = await invoke<Workflow>('update_workflow', {
          id: currentWorkflowId,
          name: trimmed,
          yamlContent: yaml,
        });
        setCurrentWorkflowId(wf.id);
      } else {
        const wf = await invoke<Workflow>('create_workflow', {
          name: trimmed,
          yamlContent: yaml,
        });
        setCurrentWorkflowId(wf.id);
      }
      setSaveDialogOpen(false);
      await refreshSaved();
    } catch (e) {
      setSaveError(String(e));
    } finally {
      setSaving(false);
    }
  };

  // 从历史列表载入一个已存 workflow 到编辑器。
  const handleLoad = async (wf: Workflow) => {
    setYaml(wf.yamlContent);
    setCurrentWorkflowId(wf.id);
    setSaveName(wf.name);
  };

  // 删除一个已存 workflow（若删的是当前，回到新建草稿态）。
  const handleDeleteSaved = async (id: string) => {
    try {
      await invoke('delete_workflow', { id });
      if (currentWorkflowId === id) setCurrentWorkflowId(null);
      await refreshSaved();
    } catch (e) {
      console.error('delete_workflow failed', e);
    }
  };

  return (
    <div className="orchestrate-view">
      {/* Header */}
      <header className="orchestrate-header">
        <h2>编排 · Workflow</h2>
        <div className="mode-tabs">
          <button
            type="button"
            className={`orch-mode-btn ${editorMode === 'visual' ? 'active' : ''}`}
            onClick={() => setEditorMode('visual')}
          >可视化</button>
          <button
            type="button"
            className={`orch-mode-btn ${editorMode === 'yaml' ? 'active' : ''}`}
            onClick={() => setEditorMode('yaml')}
          >YAML</button>
        </div>
        <span className="orchestrate-project">
          {activeProject ? activeProject.name : '未选项目'}
        </span>
        <div className="orchestrate-actions">
          <Button variant="primary" onClick={handleRun} disabled={running || !activeProject}>
            {running ? '运行中…' : '▶ 运行'}
          </Button>
          <Button variant="secondary" onClick={handleSave} disabled={running}>
            {currentWorkflowId ? '保存' : '保存为…'}
          </Button>
          <Button variant="secondary" onClick={handleSaveAs} disabled={running}>另存为</Button>
          <Button variant="secondary" onClick={reset} disabled={running}>重置</Button>
        </div>
      </header>

      {/* 保存对话框：命名后 create（首次 save / 另存为） */}
      {saveDialogOpen && (
        <div className="orch-save-dialog">
          <span className="orch-save-label">
            {saveMode === 'saveAs' ? '另存为新工作流' : '保存为新工作流'}
          </span>
          <input
            className="orch-save-input"
            type="text"
            placeholder="工作流名称"
            value={saveName}
            autoFocus
            onChange={(e) => setSaveName(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter') void doSave(saveMode, saveName);
              if (e.key === 'Escape') setSaveDialogOpen(false);
            }}
          />
          <Button variant="primary" size="sm" isLoading={saving} onClick={() => doSave(saveMode, saveName)}>
            确认保存
          </Button>
          <Button variant="secondary" size="sm" onClick={() => setSaveDialogOpen(false)}>取消</Button>
          {saveError && <span className="orch-save-error">{saveError}</span>}
        </div>
      )}

      {/* Body: main area + collapsible sidebar */}
      <div className="orch-body">
        <div className="orch-main">
          {/* Visual mode: DAG canvas (node status list) */}
          {editorMode === 'visual' ? (
            <div className="orch-dag-canvas">
              {/* WorkflowBuilder fills the canvas */}
              <WorkflowBuilder
                yaml={yaml}
                onYamlChange={setYaml}
                selectedNodeId={wfSelectedId}
                onSelectedChange={setWfSelectedId}
                onSelectedNodeChange={setWfSelectedNode}
              />

              {/* Status overlay — only when running, positioned bottom-left */}
              {running && (
                <div className="dag-status-bar">
                  <span className="dag-status-dot" style={{ background: runningNode ? 'var(--status-running)' : 'var(--gate-skip)' }} />
                  <span className="dag-status-text">
                    {nodeIds.length} 节点{runningNode ? ` · 运行中: ${runningNode}` : ''}
                  </span>
                </div>
              )}

              {/* Approval overlay */}
              {pendingApproval && (
                <div className="approval-card">
                  <strong>需要审批: {pendingApproval.node}</strong>
                  <p>{pendingApproval.prompt}</p>
                  <div className="approval-actions">
                    <Button variant="primary" onClick={() => approve(true)}>批准</Button>
                    <Button variant="secondary" onClick={() => approve(false)}>拒绝</Button>
                  </div>
                </div>
              )}
              {output != null && (
                <div className="graph-output">
                  <h4>最终输出</h4>
                  <pre>{JSON.stringify(output, null, 2)}</pre>
                </div>
              )}
              {error && <div className="graph-error">失败: {error}</div>}
            </div>
          ) : (
            /* YAML mode: raw textarea */
            <textarea
              value={yaml}
              onChange={(e) => setYaml(e.target.value)}
              spellCheck={false}
              className="yaml-editor"
            />
          )}
        </div>

        {/* Collapsible sidebar: 属性 / 节点 / 历史 */}
        <aside
          className={`orch-sidebar${sidebarCollapsed ? ' collapsed' : ''}`}
          onClick={() => sidebarCollapsed && setSidebarCollapsed(false)}
        >
          <div className="sb-tabs">
            <button type="button" className={`sb-tab ${sidebarTab === 'inspector' ? 'active' : ''}`} onClick={() => { setSidebarTab('inspector'); setSidebarCollapsed(false); }}>属性</button>
            <button type="button" className={`sb-tab ${sidebarTab === 'palette' ? 'active' : ''}`} onClick={() => { setSidebarTab('palette'); setSidebarCollapsed(false); }}>节点</button>
            <button type="button" className={`sb-tab ${sidebarTab === 'history' ? 'active' : ''}`} onClick={() => { setSidebarTab('history'); setSidebarCollapsed(false); void refreshSaved(); }}>历史</button>
            <button type="button" className="sb-collapse" onClick={() => setSidebarCollapsed(!sidebarCollapsed)} title="折叠/展开">◀</button>
          </div>
          <div className="sb-content">
            {sidebarTab === 'inspector' && (
              <div className="sb-section">
                {wfSelectedNode ? (
                  <>
                    <div className="insp-node-head">
                      <span className="insp-node-type">{wfSelectedNode.type}</span>
                      <span className="insp-node-id">{wfSelectedNode.id}</span>
                    </div>
                    <div className="insp-group">
                      <div className="insp-label">ID</div>
                      <input className="insp-input" value={wfSelectedNode.id} readOnly />
                    </div>
                    <div className="insp-group">
                      <div className="insp-label">类型</div>
                      <input className="insp-input" value={wfSelectedNode.type} readOnly />
                    </div>
                    {wfSelectedNode.agent && (
                      <div className="insp-group">
                        <div className="insp-label">Agent</div>
                        <input className="insp-input" value={wfSelectedNode.agent} readOnly />
                      </div>
                    )}
                    {wfSelectedNode.model && (
                      <div className="insp-group">
                        <div className="insp-label">模型</div>
                        <input className="insp-input" value={wfSelectedNode.model} readOnly />
                      </div>
                    )}
                    {wfSelectedNode.prompt && (
                      <div className="insp-group">
                        <div className="insp-label">提示词</div>
                        <textarea className="insp-textarea" value={wfSelectedNode.prompt} readOnly rows={4} />
                      </div>
                    )}
                    {wfSelectedNode.mode && (
                      <div className="insp-group">
                        <div className="insp-label">权限级别</div>
                        <input className="insp-input" value={wfSelectedNode.mode} readOnly />
                      </div>
                    )}
                    {wfSelectedNode.skills && wfSelectedNode.skills.length > 0 && (
                      <div className="insp-group">
                        <div className="insp-label">Skills</div>
                        <div className="kb-list">
                          {wfSelectedNode.skills.map((s) => (
                            <div key={s} className="kb-item selected">
                              <span className="kb-title">{s}</span>
                            </div>
                          ))}
                        </div>
                      </div>
                    )}
                    {wfSelectedNode.mcp_tools && wfSelectedNode.mcp_tools.length > 0 && (
                      <div className="insp-group">
                        <div className="insp-label">MCP 工具</div>
                        <div className="kb-list">
                          {wfSelectedNode.mcp_tools.map((m) => (
                            <div key={m} className="kb-item selected">
                              <span className="kb-title">{m}</span>
                            </div>
                          ))}
                        </div>
                      </div>
                    )}
                    {wfSelectedNode.knowledge && wfSelectedNode.knowledge.length > 0 && (
                      <div className="insp-group">
                        <div className="insp-label">知识库</div>
                        <div className="kb-list">
                          {wfSelectedNode.knowledge.map((k) => (
                            <div key={k} className="kb-item selected">
                              <span className="kb-title">{k}</span>
                            </div>
                          ))}
                        </div>
                      </div>
                    )}
                    <p className="muted" style={{ marginTop: 'var(--space-2)' }}>切换到 YAML 模式编辑完整配置</p>
                  </>
                ) : (
                  <p className="muted">在画布上点击一个节点查看属性</p>
                )}
              </div>
            )}
            {sidebarTab === 'palette' && (
              <div className="sb-section">
                <div className="yaml-templates">
                  <span className="yaml-templates-label">从模板开始：</span>
                  {templates.length === 0 && <span className="muted">暂无模板</span>}
                  {templates.map((t) => (
                    <Button key={t.name} variant="secondary" size="sm" title={t.description} onClick={() => setYaml(t.yamlContent)}>{t.name}</Button>
                  ))}
                </div>
              </div>
            )}
            {sidebarTab === 'history' && (
              <div className="sb-section">
                <HistoryList
                  workflows={savedWorkflows}
                  currentId={currentWorkflowId}
                  onLoad={handleLoad}
                  onDelete={handleDeleteSaved}
                  onRefresh={refreshSaved}
                />
                {runId && <p className="muted" style={{ marginTop: 'var(--space-2)' }}>当前运行: {runId.slice(0, 8)}</p>}
              </div>
            )}
          </div>
        </aside>
      </div>

      {/* Collapsible event log */}
      <div className="orch-event-log">
        <div className="event-head" onClick={() => setLogCollapsed(!logCollapsed)}>
          <span className={`event-live-dot${running ? ' running' : ''}`} />
          <h3>事件日志</h3>
          <span className={`event-chev${logCollapsed ? '' : ' open'}`}>›</span>
          <span className="event-meta">{eventLog.length} 条{runningNode ? ' · 1 运行中' : ''}</span>
        </div>
        {!logCollapsed && (
          <div className="event-body">
            {/* Node status cards — only when running or has results */}
            {(running || Object.keys(nodes).length > 0) && nodeIds.length > 0 && (
              <div className="dag-node-list" style={{marginBottom: 'var(--space-2)'}}>
                {nodeIds.map((id) => {
                  const state = nodes[id] ?? { status: 'pending' as const };
                  return (
                    <div key={id} className={`dag-node dag-node--${state.status}`} style={{ borderLeftColor: STATUS_COLOR[state.status] }}>
                      <span className="dag-node-id">{id}</span>
                      <span className="dag-node-dot" style={{ background: STATUS_COLOR[state.status] }} />
                      <span className="dag-node-status">{STATUS_LABEL[state.status]}</span>
                      {state.error && <pre className="dag-node-error">{state.error}</pre>}
                    </div>
                  );
                })}
              </div>
            )}
            {eventLog.length === 0 && <span className="muted">尚无事件</span>}
            {eventLog.map((line, i) => (
              <div key={i} className="event-line">{line}</div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/** 历史 tab：已保存 workflow 列表，载入到编辑器或删除。 */
function HistoryList({
  workflows,
  currentId,
  onLoad,
  onDelete,
  onRefresh,
}: {
  workflows: Workflow[];
  currentId: string | null;
  onLoad: (wf: Workflow) => void;
  onDelete: (id: string) => void;
  onRefresh: () => void;
}) {
  return (
    <>
      <div className="yaml-templates">
        <span className="yaml-templates-label">已保存的工作流：</span>
        <Button variant="secondary" size="sm" onClick={onRefresh}>↻ 刷新</Button>
      </div>
      {workflows.length === 0 ? (
        <p className="muted">暂无已保存工作流 — 点击「保存为…」保存当前编排</p>
      ) : (
        <ul className="wf-history-list">
          {workflows.map((wf) => (
            <li key={wf.id} className={`wf-history-item${wf.id === currentId ? ' current' : ''}`}>
              <div className="wf-history-head">
                <span className="wf-history-name" title={wf.name}>{wf.name}</span>
                <span className="wf-history-time">{formatWfTime(wf.updatedAt)}</span>
              </div>
              <div className="wf-history-actions">
                <Button variant="secondary" size="sm" onClick={() => onLoad(wf)}>载入</Button>
                <Button variant="dangerGhost" size="sm" onClick={() => onDelete(wf.id)}>删除</Button>
              </div>
            </li>
          ))}
        </ul>
      )}
    </>
  );
}

function formatWfTime(iso: string): string {
  try {
    return new Date(iso).toLocaleString();
  } catch {
    return iso;
  }
}

function formatEvent(event: WorkflowProgressPayload['event']): string {
  switch (event.kind) {
    case 'node_start':
      return `▶ ${event.node} 开始`;
    case 'node_end':
      return `${event.status === 'done' ? '✓' : event.status === 'failed' ? '✗' : '⊘'} ${event.node} ${event.error ? '— ' + event.error : ''}`;
    case 'graph_done':
      return `■ workflow 完成`;
    case 'graph_failed':
      return `■ workflow 失败: ${event.error}`;
    case 'approval_required':
      return `? ${event.node} 等待审批`;
    case 'node_output': {
      const c = event.chunk as unknown;
      let text: string;
      if (c && typeof c === 'object' && 'kind' in c) {
        const ev = c as ChatStreamEvent;
        switch (ev.kind) {
          case 'text': text = ev.content; break;
          case 'tool_use': text = `🔧 ${ev.name}`; break;
          case 'tool_result': text = ev.content; break;
          case 'result': text = ev.is_error ? '✗ 失败' : '✓ 完成'; break;
          case 'file_changed': text = `📄 ${ev.path}`; break;
          case 'thinking': text = `💭 ${ev.content.slice(0, 40)}`; break;
          default: text = JSON.stringify(c);
        }
      } else if (typeof c === 'string') {
        text = c;
      } else if (c && typeof c === 'object' && 'partial' in c) {
        text = String((c as { partial: unknown }).partial);
      } else {
        text = JSON.stringify(c);
      }
      const preview = text.length > 80 ? `${text.slice(0, 80)}…` : text;
      return `  ▸ ${event.node}: ${preview}`;
    }
    default: {
      const _exhaustive: never = event;
      return `· unhandled: ${JSON.stringify(_exhaustive)}`;
    }
  }
}
