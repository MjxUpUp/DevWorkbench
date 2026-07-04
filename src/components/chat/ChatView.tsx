import { useState, useCallback, useMemo, useRef, useEffect } from 'react';
import { useNavigationStore } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';
import type { AgentType, BranchNode, Session } from '../../types';
import { ConversationBookmarks } from './ConversationBookmarks';
import { SubagentBoard } from './SubagentBoard';
import { UserMessage } from './UserMessage';
import { AgentMessage } from './AgentMessage';
import { Composer } from './Composer';
import { ApprovalModal } from './ApprovalModal';
import { shouldFollowLatest } from './turnFollow';
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

  // 砍 CLI + 移除 agent/模式选择器后：agent 固定 Kernel Agent（唯一自研内核），执行
  // 模式不暴露给用户手切（破坏性操作由 ApprovalModal 在触发时承接，后端用默认 mode）。
  // 不再用 selectedAgent/agentMode state——canSend/handleSend 直接用常量 AGENT。
  const AGENT: AgentType = 'react_kernel';
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
  const createConversation = useAgentStore((s) => s.createConversation);
  const continueConversation = useAgentStore((s) => s.continueConversation);
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
    // Re-fetch only on conversation switch or when a new turn lands
    // (turns.length changes). Previously this depended on `allSessions`, which
    // `refreshSessions` re-creates on every token / agent event during
    // streaming — that triggered a DB-table-scan get_conversation_branches per
    // token. turns.length is the real "a new turn landed" signal; the flat
    // session-array identity is not.
    getConversationBranches(activeConversationId)
      .then((bs) => { if (!cancelled) setBranches(bs); })
      .catch(() => { if (!cancelled) setBranches([]); });
    return () => { cancelled = true; };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- turns.length is the
    // intentional re-fetch signal (new turn landed); turns itself is not needed
    // inside the effect, only its count.
  }, [activeConversationId, getConversationBranches, turns.length]);

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
    // Follow the newest turn when it extends the branch in view. visibleTurns
    // walks UP from activeLeafId, so a turn appended as a CHILD of the current
    // leaf (a natural continuation) is invisible unless the leaf advances to
    // it — which is why a follow-up in a conversation WITH history rendered
    // neither the user's message nor the agent's streaming reply until a
    // remount (a fresh conversation worked only because the leaf started null).
    // shouldFollowLatest also advances on unset/stale leaves and intentionally
    // NOT on sibling forks (A4 manual switch). See turnFollow.ts + its tests.
    if (shouldFollowLatest(turns, activeLeafId)) {
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

  const installedAgents = useMemo(() => agents.filter((a) => a.installed), [agents]);

  // Map an agent type to its display name (falls back to the raw type). Used by
  // the agent-switch divider between turns of different agents（砍 CLI 后唯一
  // react_kernel，divider 实际不会触发——保留以应对未来多内核）。
  const agentDisplayName = useCallback(
    (t: AgentType) => agents.find((a) => a.agentType === t)?.displayName ?? t.replace(/_/g, ' '),
    [agents],
  );

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
      // 砍 CLI 后唯一内核 ReactKernel——edit fork 不再按 agent family 分流。
      await editAndRegenerate(sessionId, editPrompt.trim());
      setEditingSessionId(null);
      setEditPrompt('');
    } catch (e) {
      console.error('Failed to regenerate:', e);
    }
  }, [project, editPrompt, runningSession, editAndRegenerate]);

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
  const canSend = !!project && !!prompt.trim() && !runningSession;

  // Build full prompt with attached files — use absolute paths so backend can read them
  const buildFullPrompt = useCallback(() => {
    let fullPrompt = prompt.trim();
    if (attachedFiles.length > 0) {
      const fileContext = attachedFiles.map((f) => `@${f.path}`).join(' ');
      fullPrompt = `${fileContext}\n\n${fullPrompt}`;
    }
    return fullPrompt;
  }, [prompt, attachedFiles]);

  // Local re-entry guard against the closure-stale race in handleSend. The
  // guard at the top of handleSend reads `runningSession` from the closure,
  // which is a snapshot captured when the callback was rebuilt. During the
  // `await createConversation(...)` window the store flips runningSession to
  // non-empty (agent:started), but the still-resident closure sees the old
  // null and a second click would slip past the guard → two spawn_agent_session
  // calls → two turns. A ref is read live (not from the closure), so it's an
  // atomic per-instance lock independent of when the callback was last rebuilt.
  const sendingRef = useRef(false);
  const handleSend = useCallback(async () => {
    if (sendingRef.current) return;
    if (!prompt.trim() || runningSession || !project) return;
    sendingRef.current = true;
    const text = buildFullPrompt();
    try {
      const model = selectedModel && selectedModel !== 'default' ? selectedModel : undefined;
      if (activeConversationId && !runningSession) {
        const parentSessionId = turns.length > 0 ? turns[turns.length - 1].id : undefined;
        const session = await continueConversation(project.path, activeConversationId, text, AGENT, undefined, model, parentSessionId);
        void session;
        // Force store sync to guarantee ChatView re-render with new turn
        void useAgentStore.getState().refreshSessions();
      } else {
        const session = await createConversation(project.path, text, AGENT, undefined, model);
        selectConversation(session.conversationId);
        // Force store sync to guarantee ChatView re-render with new turn
        void useAgentStore.getState().refreshSessions();
      }
      setPrompt('');
      setAttachedFiles([]);
    } catch (e) {
      console.error('Failed to send:', e);
      toast.error(`发送失败: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
      sendingRef.current = false;
    }
  }, [prompt, runningSession, project, activeConversationId, turns, createConversation, continueConversation, selectConversation, buildFullPrompt, selectedModel, toast]);

  const handleStop = useCallback(async () => {
    if (!runningSession) return;
    try { await stopAgent(runningSession.id); } catch (e) { console.error('Failed to stop:', e); toast.error(`停止失败: ${e instanceof Error ? e.message : String(e)}`); }
  }, [runningSession, stopAgent, toast]);

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
            从底部选择工作区，或点击「+」添加新工作区开始任务
          </p>
          {installedAgents.length > 0 && (
            <p style={{ fontSize: 'var(--text-xs)', color: 'var(--text-tertiary)' }}>
              内核 Agent 已就绪：{installedAgents.map((a) => a.displayName).join('、')}
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
  // 会话切换/新增/删除由常驻顶部的 ConversationBookmarks 承接（替代旧 SessionStartCards
  // 两张卡片 + ChatHeader 的清空按钮——书签栏的 + 新建 = selectConversation(null)）。
  if (!runningSession && turns.length === 0) {
    return (
      <div className="chat-view">
        <ConversationBookmarks project={project} requestId={activeConversationId ?? undefined} running={false} />
        <div style={{ padding: 'var(--space-4) var(--space-6)', color: 'var(--text-tertiary)', fontSize: 'var(--text-sm)' }} data-testid="session-new-hint">
          在下方输入需求或指令开始新会话，或点上方书签继续旧会话
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
          selectedModel={selectedModel}
          onModelChange={setSelectedModel}
          modelOptions={modelOptions}
          placeholder="提出后续修改要求... @ 文件 / 命令 $ 技能"
        />
      </div>
    );
  }

  // Active conversation — show its turn stream
  return (
    <div className="chat-view">
      <ConversationBookmarks project={project} requestId={runningSession?.id ?? activeConversationId ?? undefined} running={!!runningSession} />
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
        selectedModel={selectedModel}
        onModelChange={setSelectedModel}
        modelOptions={modelOptions}
        steering={!!runningSession}
        onSteer={() => toast.info('Steering 消息发送待后端支持（当前会继续运行）')}
        placeholder={isContinuing ? '提出后续修改要求... @ 文件 / 命令 $ 技能' : '输入需求或指令... @ 文件 / 命令 $ 技能'}
      />
      <ApprovalModal />
    </div>
  );
}
