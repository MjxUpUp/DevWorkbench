import { useState, useCallback, useMemo, useRef, useEffect } from 'react';
import { useNavigationStore } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';
import type { AgentType, BranchNode, Session } from '../../types';
import type { AgentMode } from '../ModeSelector';
import { ChatHeader } from './ChatHeader';
import { SubagentBoard } from './SubagentBoard';
import { UserMessage } from './UserMessage';
import { AgentMessage } from './AgentMessage';
import { Composer } from './Composer';
import { useProvidersStore } from '../../stores/providersStore';
import type { ModelOption } from '../ModelSelector';
import { useToast } from '../Toast';

interface AttachedFile {
  path: string;
  name: string;
}

export function ChatView() {
  const project = useNavigationStore((s) => s.activeProject);
  const activeConversationId = useNavigationStore((s) => s.selectedConversationId);
  const selectConversation = useNavigationStore((s) => s.selectConversation);
  const toast = useToast();

  // Agent state — the selected agent applies to the NEXT turn. Because turns
  // can switch agents within one conversation, this is per-send, not per-conversation.
  const [selectedAgent, setSelectedAgent] = useState<AgentType | null>(null);
  const [agentMode, setAgentMode] = useState<AgentMode>('default');
  const [selectedModel, setSelectedModel] = useState('default');
  // Providers config (providers.toml) — the source of truth for the model
  // picker, so the chat selector lists the SAME models Settings → Providers
  // configures (was a hardcoded 4-item list → "列表对不上" the config page).
  // Loaded once on mount; selecting a non-default id routes through the matching
  // enabled provider — that's how "切换供应商" works (one model per provider).
  const providersConfig = useProvidersStore((s) => s.config);
  const loadProviders = useProvidersStore((s) => s.loadProviders);
  useEffect(() => { void loadProviders(); }, [loadProviders]);
  const modelOptions: ModelOption[] = useMemo(() => {
    const opts: ModelOption[] = [{ id: 'default', label: '默认模型', provider: '系统' }];
    for (const p of providersConfig?.providers ?? []) {
      if (!p.enabled) continue;
      for (const m of p.models) {
        if (!m.enabled) continue;
        opts.push({ id: m.id, label: m.label || m.id, provider: p.name });
      }
    }
    return opts;
  }, [providersConfig]);
  const [prompt, setPrompt] = useState('');
  const [attachedFiles, setAttachedFiles] = useState<AttachedFile[]>([]);

  // Stores
  const allSessions = useAgentStore((s) => s.sessions);
  const agents = useAgentStore((s) => s.agents);
  const stopAgent = useAgentStore((s) => s.stopAgent);
  const getTurnsForConversation = useAgentStore((s) => s.getTurnsForConversation);
  const recommendAgent = useAgentStore((s) => s.recommendAgent);
  const createConversation = useAgentStore((s) => s.createConversation);
  const continueConversation = useAgentStore((s) => s.continueConversation);
  const getDefaultAgent = useAgentStore((s) => s.getDefaultAgent);
  const qualityReports = useAgentStore((s) => s.qualityReports);
  const fetchQualityReport = useAgentStore((s) => s.fetchQualityReport);

  // Turns of the active conversation, oldest-first. Empty when nothing selected.
  const turns = useMemo(
    () => (activeConversationId ? getTurnsForConversation(activeConversationId) : []),
    [activeConversationId, getTurnsForConversation, allSessions]
  );

  // A4 edit-and-regenerate: branch-aware view. A forked (regenerated) turn is a
  // SIBLING under the edited turn's parent, so the flat turn list would show two
  // branches at once. We render ONE branch chain at a time (root → activeLeaf)
  // and switch between siblings via the branch switcher. Linear conversations
  // have a single child per parent, so the chain == the flat list (no change).
  const getConversationBranches = useAgentStore((s) => s.getConversationBranches);
  const editAndRegenerate = useAgentStore((s) => s.editAndRegenerate);
  const [branches, setBranches] = useState<BranchNode[]>([]);
  useEffect(() => {
    if (!activeConversationId) { setBranches([]); return; }
    let cancelled = false;
    // Re-fetch whenever the session set changes (a new fork/turn lands) so the
    // switcher reflects the latest siblings.
    getConversationBranches(activeConversationId)
      .then((bs) => { if (!cancelled) setBranches(bs); })
      .catch(() => { if (!cancelled) setBranches([]); });
    return () => { cancelled = true; };
  }, [activeConversationId, getConversationBranches, allSessions]);

  // parent_id → children (siblings grouped). null key = root-level turns.
  const childrenByParent = useMemo(() => {
    const m = new Map<string | null, BranchNode[]>();
    for (const b of branches) {
      const arr = m.get(b.parentId) ?? [];
      arr.push(b);
      m.set(b.parentId, arr);
    }
    return m;
  }, [branches]);

  // activeLeaf = the bottom of the branch currently shown. Defaults to the
  // newest turn; a fork or follow-up lands a newer turn, so we follow it.
  const [activeLeafId, setActiveLeafId] = useState<string | null>(null);
  const latestTurnId = turns.length > 0 ? turns[turns.length - 1].id : null;
  useEffect(() => {
    if (latestTurnId && (!activeLeafId || !turns.some((t) => t.id === activeLeafId))) {
      setActiveLeafId(latestTurnId);
    }
  }, [latestTurnId, activeLeafId, turns]);

  // Walk from activeLeaf up the parent chain to the root → the visible branch.
  const sessionById = useMemo(() => {
    const m = new Map<string, Session>();
    for (const t of turns) m.set(t.id, t);
    return m;
  }, [turns]);
  const visibleTurns = useMemo(() => {
    if (!activeLeafId) return turns;
    const chain: Session[] = [];
    let cursor: string | null = activeLeafId;
    const visited = new Set<string>();
    while (cursor) {
      if (visited.has(cursor)) break;
      visited.add(cursor);
      const s = sessionById.get(cursor);
      if (!s) break;
      chain.push(s);
      cursor = s.parentSessionId;
    }
    chain.reverse();
    return chain;
  }, [activeLeafId, turns, sessionById]);

  // The running turn is whichever turn of the active conversation is still running.
  const runningSession = useMemo(
    () => turns.find((s) => s.status === 'running') ?? null,
    [turns]
  );

  const displaySession = runningSession ?? turns[turns.length - 1] ?? null;

  // Auto-select agent on mount
  const installedAgents = useMemo(() => agents.filter((a) => a.installed), [agents]);

  // Reset agent selection when switching projects. Without this, the prior
  // project's recommended agent lingers in local state and the <select> can
  // render with a value that no longer matches its options — the "selector
  // loses its options on project switch" symptom. Resetting to null lets the
  // recommend-effect below re-pick for the new project.
  const prevProjectPath = useRef<string | null>(project?.path ?? null);
  useEffect(() => {
    const currentPath = project?.path ?? null;
    if (prevProjectPath.current !== currentPath) {
      prevProjectPath.current = currentPath;
      setSelectedAgent(null);
    }
  }, [project?.path]);

  useEffect(() => {
    if (selectedAgent) return;
    const tags = project?.tags ?? [];
    recommendAgent(tags).then((rec) => {
      if (rec && agents.find((a) => a.agentType === rec)?.installed) {
        setSelectedAgent(rec);
      } else if (installedAgents.length > 0) {
        setSelectedAgent(installedAgents[0].agentType);
      }
    }).catch(() => {
      if (installedAgents.length > 0) {
        setSelectedAgent(installedAgents[0].agentType);
      }
    });
  }, [project, agents, installedAgents, selectedAgent, recommendAgent]);

  // Map an agent type to its display name (falls back to the raw type). Used by
  // the agent-switch divider between turns of different agents.
  const agentDisplayName = useCallback(
    (t: AgentType) => agents.find((a) => a.agentType === t)?.displayName ?? t.replace(/_/g, ' '),
    [agents],
  );

  // When the selected conversation already has turns, default the agent picker
  // to the last turn's agent so a follow-up feels continuous (user can still
  // change it — switching agents mid-conversation is the point).
  useEffect(() => {
    if (turns.length > 0 && !runningSession) {
      const last = turns[turns.length - 1];
      setSelectedAgent(last.agentType);
    }
  }, [activeConversationId, turns, runningSession]);

  // Fetch quality report when session completes
  const qualityReport = useMemo(() => {
    if (!displaySession || displaySession.status === 'running') return null;
    return qualityReports.get(displaySession.id) ?? null;
  }, [displaySession, qualityReports]);

  useEffect(() => {
    if (displaySession && displaySession.status !== 'running' && !qualityReport) {
      fetchQualityReport(displaySession.id);
    }
  }, [displaySession?.id, displaySession?.status, qualityReport, fetchQualityReport]);

  // Elapsed timer for running session
  const [elapsed, setElapsed] = useState('');
  useEffect(() => {
    if (!runningSession) { setElapsed(''); return; }
    const start = new Date(runningSession.startedAt).getTime();
    const update = () => {
      const sec = Math.floor((Date.now() - start) / 1000);
      const m = Math.floor(sec / 60);
      const s = sec % 60;
      setElapsed(m > 0 ? `${m}:${String(s).padStart(2, '0')}` : `${s}s`);
    };
    update();
    const id = setInterval(update, 1000);
    return () => clearInterval(id);
  }, [runningSession]);

  // A4 edit-and-regenerate handlers. Editing a turn's prompt forks a new
  // sibling turn (same conversation, parent = the edited turn's parent) and
  // re-runs the agent — the old turn stays, switchable via the branch switcher.
  const [editingSessionId, setEditingSessionId] = useState<string | null>(null);
  const [editPrompt, setEditPrompt] = useState('');

  const startEdit = useCallback((sessionId: string, currentPrompt: string) => {
    setEditingSessionId(sessionId);
    setEditPrompt(currentPrompt);
  }, []);

  const handleEditSubmit = useCallback(async (sessionId: string) => {
    if (!project || !editPrompt.trim() || runningSession) return;
    try {
      // kernel flag mirrors the edited turn's agent family — react_kernel runs
      // the self-hosted ReactAgent; others fork through their CLI spawn path.
      const edited = sessionById.get(sessionId);
      const kernel = edited?.agentType === 'react_kernel';
      await editAndRegenerate(sessionId, editPrompt.trim(), kernel);
      setEditingSessionId(null);
      setEditPrompt('');
    } catch (e) {
      console.error('Failed to regenerate:', e);
    }
  }, [project, editPrompt, runningSession, editAndRegenerate, sessionById]);

  // Branch switching: jump to the next sibling's deepest leaf. Bounded walk
  // with a visited guard so a malformed cycle can't hang the render.
  const deepestLeaf = useCallback((id: string): string => {
    let cur = id;
    const seen = new Set<string>();
    for (let i = 0; i < 1000; i++) {
      if (seen.has(cur)) break;
      seen.add(cur);
      const children = childrenByParent.get(cur);
      if (!children || children.length === 0) break;
      const newest = [...children].sort(
        (a, b) => new Date(b.startedAt).getTime() - new Date(a.startedAt).getTime(),
      )[0];
      cur = newest.id;
    }
    return cur;
  }, [childrenByParent]);

  const switchToSibling = useCallback((turnId: string, parentId: string | null) => {
    const siblings = childrenByParent.get(parentId) ?? [];
    if (siblings.length <= 1) return;
    const idx = siblings.findIndex((s) => s.id === turnId);
    if (idx < 0) return;
    const next = siblings[(idx + 1) % siblings.length];
    setActiveLeafId(deepestLeaf(next.id));
  }, [childrenByParent, deepestLeaf]);

  const isContinuing = turns.length > 0 && !runningSession;
  const canSend = !!project && !!selectedAgent && !!prompt.trim() && !runningSession;

  // Build full prompt with attached files — use absolute paths so backend can read them
  const buildFullPrompt = useCallback(() => {
    let fullPrompt = prompt.trim();
    if (attachedFiles.length > 0) {
      const fileContext = attachedFiles.map((f) => `@${f.path}`).join(' ');
      fullPrompt = `${fileContext}\n\n${fullPrompt}`;
    }
    return fullPrompt;
  }, [prompt, attachedFiles]);

  const handleSend = useCallback(async () => {
    if (!selectedAgent || !prompt.trim() || runningSession || !project) return;
    const text = buildFullPrompt();
    try {
      // Kernel agents (self-hosted ReactAgent) route through react_chat_driver
      // instead of a CLI subprocess — flagged by kernel=true.
      const kernel = selectedAgent === 'react_kernel';
      // Resolve the model for the backend: 'default'/unset → let the backend
      // pick; any concrete id routes through the matching enabled provider.
      // Without this the picker was decorative — the value never reached
      // spawn_agent_session (logged model=None → fallback or send failure).
      const model = selectedModel && selectedModel !== 'default' ? selectedModel : undefined;
      if (activeConversationId && !runningSession) {
        // Follow-up turn on the existing conversation. The agent may differ
        // from the previous turn — that's the conversation-container model.
        // parentSessionId links this follow-up to the prior turn — the backbone
        // of branch-aware history (visibleTurns walks the chain; edit_and_regenerate
        // forks off it). Undefined for the very first turn of the container.
        const parentSessionId = turns.length > 0 ? turns[turns.length - 1].id : undefined;
        const session = await continueConversation(project.path, activeConversationId, text, selectedAgent, kernel, agentMode, model, parentSessionId);
        // continueConversation attaches to the already-selected conversation;
        // selection is already correct, no need to re-select.
        void session;
      } else {
        // First turn of a brand-new conversation. createConversation spawns
        // turn 1 and returns it carrying the new conversationId; select it so
        // the main view binds to the new container.
        const agent = selectedAgent || getDefaultAgent();
        if (agent) {
          const session = await createConversation(project.path, text, agent, kernel, agentMode, model);
          selectConversation(session.conversationId);
        }
      }
      setPrompt('');
      setAttachedFiles([]);
    } catch (e) {
      console.error('Failed to send:', e);
      toast.error(`发送失败: ${e instanceof Error ? e.message : String(e)}`);
    }
  }, [selectedAgent, prompt, runningSession, project, activeConversationId, turns, createConversation, continueConversation, getDefaultAgent, selectConversation, buildFullPrompt, agentMode, selectedModel, toast]);

  const handleStop = useCallback(async () => {
    if (!runningSession) return;
    try { await stopAgent(runningSession.id); } catch (e) { console.error('Failed to stop:', e); toast.error(`停止失败: ${e instanceof Error ? e.message : String(e)}`); }
  }, [runningSession, stopAgent, toast]);

  const handleClear = useCallback(() => {
    // "New conversation" intent — drop the selection so the empty-state shows
    // and the next send starts a fresh container. Does NOT delete history.
    selectConversation(null);
    setPrompt('');
    setAttachedFiles([]);
  }, [selectConversation]);

  const handleAttachFile = useCallback((file: AttachedFile) => {
    setAttachedFiles((prev) => [...prev, file]);
  }, []);

  const handleRemoveFile = useCallback((path: string) => {
    setAttachedFiles((prev) => prev.filter((f) => f.path !== path));
  }, []);

  // Auto-scroll message list
  const messageListRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (messageListRef.current) {
      messageListRef.current.scrollTop = messageListRef.current.scrollHeight;
    }
  }, [visibleTurns, runningSession]);

  // Landing state — no project selected
  if (!project) {
    return (
      <div className="chat-view">
        <div className="chat-empty">
          <div className="chat-empty-icon">DW</div>
          <h2>Dev Workbench</h2>
          <p>多 Agent 编码操作系统</p>
          <p style={{ fontSize: 'var(--text-sm)', color: 'var(--text-tertiary)' }}>
            从左侧选择项目开始创建任务
          </p>
          {installedAgents.length > 0 && (
            <p style={{ fontSize: 'var(--text-xs)', color: 'var(--text-tertiary)' }}>
              已检测 Agent：{installedAgents.map((a) => a.displayName).join('、')}
            </p>
          )}
        </div>
      </div>
    );
  }

  // Empty state — project selected, no conversation selected (or selected but
  // has no turns yet) AND nothing running. This is the "type your first turn"
  // surface. Keying off turns (actual content), NOT activeConversationId:
  // selectProject clears the selection on every project switch, so a project
  // with history would otherwise flash the empty state when revisited.
  if (!runningSession && turns.length === 0) {
    return (
      <div className="chat-view">
        <ChatHeader
          selectedAgent={selectedAgent}
          onAgentChange={setSelectedAgent}
          agentMode={agentMode}
          onModeChange={setAgentMode}
          selectedModel={selectedModel}
          onModelChange={setSelectedModel}
          modelOptions={modelOptions}
          onClear={handleClear}
        />
        <div className="chat-empty">
          <div style={{ fontSize: 32, marginBottom: 'var(--space-2)' }}>✦</div>
          <h2>创建任务</h2>
          <p>在下方输入需求或指令，开始与 Agent 协作</p>
        </div>
        <Composer
          prompt={prompt}
          onPromptChange={setPrompt}
          onSend={handleSend}
          onStop={handleStop}
          canSend={canSend}
          isRunning={false}
          attachedFiles={attachedFiles}
          onAttachFile={handleAttachFile}
          onRemoveFile={handleRemoveFile}
          agentMode={agentMode}
          onModeChange={setAgentMode}
          placeholder="提出后续修改要求... @ 文件 / 命令 $ 技能"
        />
      </div>
    );
  }

  // Active conversation — show its turn stream
  return (
    <div className="chat-view">
      <ChatHeader
        selectedAgent={selectedAgent}
        onAgentChange={setSelectedAgent}
        agentMode={agentMode}
        onModeChange={setAgentMode}
        selectedModel={selectedModel}
        onModelChange={setSelectedModel}
        modelOptions={modelOptions}
        onClear={handleClear}
      />
      <SubagentBoard
        events={
          visibleTurns.length
            ? visibleTurns[visibleTurns.length - 1].blocks ?? null
            : null
        }
      />
      <div className="message-list" ref={messageListRef}>
        {visibleTurns.map((session, i) => {
          // Insert a divider before a turn whose agent differs from the previous
          // turn — this is the visible cue that the conversation switched agents
          // (e.g. claude → codex). The first turn never gets one.
          const prev = i > 0 ? visibleTurns[i - 1] : null;
          const switchedFrom = prev && prev.agentType !== session.agentType
            ? prev.agentType
            : null;
          // A4: siblings of THIS turn (same parent). >1 ⇒ a branch point — the
          // switcher lets the user walk between regenerated forks.
          const siblings = childrenByParent.get(session.parentSessionId) ?? [];
          const branchCount = siblings.length;
          const branchIndex = siblings.findIndex((s) => s.id === session.id);
          const isEditing = editingSessionId === session.id;
          return (
            <div key={session.id}>
              {switchedFrom && (
                <div className="agent-switch-divider" role="separator" aria-label="切换 Agent">
                  <span className="agent-switch-divider-line" />
                  <span className="agent-switch-divider-label">
                    {agentDisplayName(switchedFrom)} → {agentDisplayName(session.agentType)}
                  </span>
                  <span className="agent-switch-divider-line" />
                </div>
              )}
              {isEditing ? (
                <div className="user-message-edit">
                  <textarea
                    className="user-message-edit-textarea"
                    value={editPrompt}
                    onChange={(e) => setEditPrompt(e.target.value)}
                    rows={3}
                    aria-label="编辑消息"
                  />
                  <div className="user-message-edit-actions">
                    <button
                      type="button"
                      className="user-message-edit-submit"
                      onClick={() => handleEditSubmit(session.id)}
                      disabled={!editPrompt.trim() || !!runningSession}
                    >
                      重新生成
                    </button>
                    <button
                      type="button"
                      className="user-message-edit-cancel"
                      onClick={() => { setEditingSessionId(null); setEditPrompt(''); }}
                    >
                      取消
                    </button>
                  </div>
                </div>
              ) : (
                <div className="user-message-wrap">
                  <UserMessage content={session.prompt} />
                  <div className="turn-actions">
                    <button
                      type="button"
                      className="turn-edit-btn"
                      onClick={() => startEdit(session.id, session.prompt)}
                      disabled={!!runningSession}
                      title="编辑并重新生成"
                      aria-label="编辑并重新生成"
                    >
                      ✎ 编辑
                    </button>
                    {branchCount > 1 && (
                      <button
                        type="button"
                        className="branch-switch-btn"
                        onClick={() => switchToSibling(session.id, session.parentSessionId)}
                        title={`切换分支(共 ${branchCount} 个)`}
                        aria-label={`切换分支 ${branchIndex + 1} / ${branchCount}`}
                      >
                        ↥ 分支 {branchIndex + 1}/{branchCount}
                      </button>
                    )}
                  </div>
                </div>
              )}
              <AgentMessage
                session={session}
                running={runningSession?.id === session.id}
                qualityReport={qualityReports.get(session.id) ?? null}
                elapsed={runningSession?.id === session.id ? elapsed : undefined}
              />
            </div>
          );
        })}
      </div>
      <Composer
        prompt={prompt}
        onPromptChange={setPrompt}
        onSend={handleSend}
        onStop={handleStop}
        canSend={canSend}
        isRunning={!!runningSession}
        attachedFiles={attachedFiles}
        onAttachFile={handleAttachFile}
        onRemoveFile={handleRemoveFile}
        agentMode={agentMode}
        onModeChange={setAgentMode}
        placeholder={isContinuing ? '提出后续修改要求... @ 文件 / 命令 $ 技能' : '输入需求或指令... @ 文件 / 命令 $ 技能'}
      />
    </div>
  );
}
