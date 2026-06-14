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
  const activeSessionId = useNavigationStore((s) => s.selectedSessionId);
  const selectSession = useNavigationStore((s) => s.selectSession);

  // Agent state — lifted from old AgentPanel to ChatView level
  const [selectedAgent, setSelectedAgent] = useState<AgentType | null>(null);
  const [agentMode, setAgentMode] = useState<AgentMode>('default');
  const [selectedModel, setSelectedModel] = useState('default');
  const [prompt, setPrompt] = useState('');
  const [attachedFiles, setAttachedFiles] = useState<AttachedFile[]>([]);

  // Stores
  const allSessions = useAgentStore((s) => s.sessions);
  const agents = useAgentStore((s) => s.agents);
  const spawnAgent = useAgentStore((s) => s.spawnAgent);
  const stopAgent = useAgentStore((s) => s.stopAgent);
  const getSessionsForProject = useAgentStore((s) => s.getSessionsForProject);
  const recommendAgent = useAgentStore((s) => s.recommendAgent);
  const newConversation = useAgentStore((s) => s.newConversation);
  const getDefaultAgent = useAgentStore((s) => s.getDefaultAgent);
  const qualityReports = useAgentStore((s) => s.qualityReports);
  const fetchQualityReport = useAgentStore((s) => s.fetchQualityReport);

  const projectSessions = useMemo(
    () => project ? getSessionsForProject(project.path) : [],
    [getSessionsForProject, project?.path, allSessions]
  );

  const runningSession = useMemo(
    () => projectSessions.find((s) => s.status === 'running') ?? null,
    [projectSessions]
  );

  const displaySession = runningSession ?? (
    activeSessionId ? allSessions.find((s) => s.id === activeSessionId) ?? null : null
  );

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

  // All sessions for the project, sorted by time (for message list)
  const messageSessions = useMemo(() => {
    return [...projectSessions].sort((a, b) =>
      new Date(a.startedAt).getTime() - new Date(b.startedAt).getTime()
    );
  }, [projectSessions]);

  const isContinuing = !!displaySession && displaySession.status !== 'running';
  const hasConversation = !!(activeSessionId || runningSession);
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
      if (activeSessionId && displaySession && displaySession.status !== 'running') {
        // Continuing a prior conversation — spawn a follow-up session linked
        // to the active one via parentSessionId. (Requirement concept removed:
        // the dialogue itself is the task.)
        const session = await spawnAgent(
          project.path, selectedAgent, text,
          undefined,
          undefined,
          activeSessionId
        );
        selectSession(session.id);
      } else {
        const agent = selectedAgent || getDefaultAgent();
        if (agent) {
          const session = await newConversation(project.path, text, agent);
          selectSession(session.id);
        }
      }
      setPrompt('');
      setAttachedFiles([]);
    } catch (e) {
      console.error('Failed to send:', e);
    }
  }, [selectedAgent, prompt, runningSession, project, spawnAgent, activeSessionId, displaySession, newConversation, getDefaultAgent, selectSession, buildFullPrompt]);

  const handleStop = useCallback(async () => {
    if (!runningSession) return;
    try { await stopAgent(runningSession.id); } catch (e) { console.error('Failed to stop:', e); }
  }, [runningSession, stopAgent]);

  const handleClear = useCallback(() => {
    selectSession(null);
    setPrompt('');
    setAttachedFiles([]);
  }, [selectSession]);

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
  }, [messageSessions, runningSession]);

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

  // Empty state — project selected but no sessions
  if (!hasConversation && !runningSession) {
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

  // Active conversation — show message list
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
        {messageSessions.map((session) => (
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
