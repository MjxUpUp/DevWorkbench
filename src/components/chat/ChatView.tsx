import { useState, useCallback, useMemo, useRef, useEffect } from 'react';
import { useNavigationStore } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';
import type { AgentType } from '../../types';
import type { AgentMode } from '../ModeSelector';
import { ChatHeader } from './ChatHeader';
import { UserMessage } from './UserMessage';
import { AgentMessage } from './AgentMessage';
import { Composer } from './Composer';

interface AttachedFile {
  path: string;
  name: string;
}

export function ChatView() {
  const project = useNavigationStore((s) => s.activeProject);
  const activeConversationId = useNavigationStore((s) => s.selectedConversationId);
  const selectConversation = useNavigationStore((s) => s.selectConversation);

  // Agent state — the selected agent applies to the NEXT turn. Because turns
  // can switch agents within one conversation, this is per-send, not per-conversation.
  const [selectedAgent, setSelectedAgent] = useState<AgentType | null>(null);
  const [agentMode, setAgentMode] = useState<AgentMode>('default');
  const [selectedModel, setSelectedModel] = useState('default');
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
      if (activeConversationId && !runningSession) {
        // Follow-up turn on the existing conversation. The agent may differ
        // from the previous turn — that's the conversation-container model.
        const session = await continueConversation(project.path, activeConversationId, text, selectedAgent);
        // continueConversation attaches to the already-selected conversation;
        // selection is already correct, no need to re-select.
        void session;
      } else {
        // First turn of a brand-new conversation. createConversation spawns
        // turn 1 and returns it carrying the new conversationId; select it so
        // the main view binds to the new container.
        const agent = selectedAgent || getDefaultAgent();
        if (agent) {
          const session = await createConversation(project.path, text, agent);
          selectConversation(session.conversationId);
        }
      }
      setPrompt('');
      setAttachedFiles([]);
    } catch (e) {
      console.error('Failed to send:', e);
    }
  }, [selectedAgent, prompt, runningSession, project, activeConversationId, createConversation, continueConversation, getDefaultAgent, selectConversation, buildFullPrompt]);

  const handleStop = useCallback(async () => {
    if (!runningSession) return;
    try { await stopAgent(runningSession.id); } catch (e) { console.error('Failed to stop:', e); }
  }, [runningSession, stopAgent]);

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
  }, [turns, runningSession]);

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
          selectedModel={selectedModel}
          onModelChange={setSelectedModel}
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
        onClear={handleClear}
      />
      <div className="message-list" ref={messageListRef}>
        {turns.map((session) => (
          <div key={session.id}>
            <UserMessage content={session.prompt} />
            <AgentMessage
              session={session}
              running={runningSession?.id === session.id}
              qualityReport={qualityReports.get(session.id) ?? null}
              elapsed={runningSession?.id === session.id ? elapsed : undefined}
            />
          </div>
        ))}
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
        selectedModel={selectedModel}
        onModelChange={setSelectedModel}
        placeholder={isContinuing ? '提出后续修改要求... @ 文件 / 命令 $ 技能' : '输入需求或指令... @ 文件 / 命令 $ 技能'}
      />
    </div>
  );
}
