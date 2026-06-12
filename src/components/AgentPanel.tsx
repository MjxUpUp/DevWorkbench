import { useState, useCallback, useMemo, useRef, useEffect } from 'react';
import type { AgentType } from '../types';
import { TerminalView } from './TerminalView';
import { QualityReportPanel } from './QualityReportPanel';
import { QualityBadge } from './QualityBadge';
import { IconPlay, IconStop } from './Icons';
import { TriggerMenu } from './TriggerMenu';
import { ModeSelector, type AgentMode } from './ModeSelector';
import { ModelSelector } from './ModelSelector';
import { useAgentStore } from '../stores/agentStore';
import { useNavigationStore } from '../stores/navigationStore';

interface AttachedFile {
  path: string;
  name: string;
}

export function AgentPanel() {
  const project = useNavigationStore((s) => s.activeProject);
  const activeSessionId = useNavigationStore((s) => s.selectedSessionId);
  const selectSession = useNavigationStore((s) => s.selectSession);
  const [selectedAgent, setSelectedAgent] = useState<AgentType | null>(null);
  const [prompt, setPrompt] = useState('');
  const [recommended, setRecommended] = useState<AgentType | null>(null);
  const [showAgentDropdown, setShowAgentDropdown] = useState(false);
  const [agentMode, setAgentMode] = useState<AgentMode>('default');
  const [selectedModel, setSelectedModel] = useState('default');
  const [attachedFiles, setAttachedFiles] = useState<AttachedFile[]>([]);
  const [triggerMenu, setTriggerMenu] = useState<{ type: '@' | '/' | '$'; position: { top: number; left: number } } | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement>(null);

  // Zustand stores
  const allSessions = useAgentStore((s) => s.sessions);
  const agents = useAgentStore((s) => s.agents);
  const spawnAgent = useAgentStore((s) => s.spawnAgent);
  const stopAgent = useAgentStore((s) => s.stopAgent);
  const getSessionsForProject = useAgentStore((s) => s.getSessionsForProject);
  const recommendAgent = useAgentStore((s) => s.recommendAgent);
  const newConversation = useAgentStore((s) => s.newConversation);
  const launchForRequirement = useAgentStore((s) => s.launchForRequirement);
  const deleteRequirement = useAgentStore((s) => s.removeRequirement);
  const updateRequirement = useAgentStore((s) => s.updateRequirement);
  const getDefaultAgent = useAgentStore((s) => s.getDefaultAgent);

  const allRequirements = useAgentStore((s) => s.requirements);
  const requirements = useMemo(
    () => project ? allRequirements.filter(r => r.projectPath === project.path) : [],
    [allRequirements, project?.path]
  );

  const projectSessions = useMemo(
    () => project ? getSessionsForProject(project.path) : [],
    [getSessionsForProject, project?.path, allSessions]
  );

  const runningSession = useMemo(
    () => projectSessions.find(s => s.status === 'running') ?? null,
    [projectSessions]
  );

  // Find the active requirement linked to session
  const activeRequirement = useMemo(() => {
    if (activeSessionId) {
      return requirements.find(r => r.linkedSessionId === activeSessionId) ?? null;
    }
    return null;
  }, [requirements, activeSessionId]);

  // The session to display
  const displaySession = runningSession ?? (
    activeSessionId ? allSessions.find(s => s.id === activeSessionId) ?? null : null
  );

  const installedAgents = useMemo(
    () => agents.filter(a => a.installed),
    [agents]
  );

  // Fetch quality report when a session completes
  const qualityReports = useAgentStore((s) => s.qualityReports);
  const fetchQualityReport = useAgentStore((s) => s.fetchQualityReport);
  const qualityReport = useMemo(() => {
    if (!displaySession || displaySession.status === 'running') return null;
    return qualityReports.get(displaySession.id) ?? null;
  }, [displaySession, qualityReports]);

  useEffect(() => {
    if (displaySession && displaySession.status !== 'running' && !qualityReport) {
      fetchQualityReport(displaySession.id);
    }
  }, [displaySession?.id, displaySession?.status, qualityReport, fetchQualityReport]);

  // Auto-select first installed agent or recommended
  useEffect(() => {
    if (selectedAgent) return;
    const tags = project?.tags ?? [];
    recommendAgent(tags).then(rec => {
      setRecommended(rec);
      if (rec && agents.find(a => a.agentType === rec)?.installed) {
        setSelectedAgent(rec);
      } else if (installedAgents.length > 0) {
        setSelectedAgent(installedAgents[0].agentType);
      }
    });
  }, [project]);

  const isContinuing = !!displaySession && displaySession.status !== 'running';

  // Build prompt with attached files
  const buildFullPrompt = useCallback(() => {
    let fullPrompt = prompt.trim();
    if (attachedFiles.length > 0) {
      const fileContext = attachedFiles.map(f => `@${f.path}`).join(' ');
      fullPrompt = `${fileContext}\n\n${fullPrompt}`;
    }
    return fullPrompt;
  }, [prompt, attachedFiles]);

  const handleSend = useCallback(async () => {
    if (!selectedAgent || !prompt.trim() || runningSession || !project) return;
    const text = buildFullPrompt();
    try {
      if (activeSessionId && displaySession && displaySession.status !== 'running') {
        // Continue: link new session to same requirement + switch view
        const linkedReq = requirements.find(r => r.linkedSessionId === activeSessionId);
        const session = await spawnAgent(
          project.path, selectedAgent, text,
          selectedModel !== 'default' ? selectedModel : undefined,
          linkedReq?.id,
          activeSessionId
        );
        if (linkedReq) {
          await updateRequirement(linkedReq.id, {
            linkedSessionId: session.id,
            updatedAt: new Date().toISOString(),
          });
        }
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
  }, [selectedAgent, prompt, runningSession, project, spawnAgent, activeSessionId, displaySession, requirements, newConversation, getDefaultAgent, selectSession, updateRequirement, buildFullPrompt, selectedModel]);

  const handleStop = useCallback(async () => {
    if (!runningSession) return;
    try {
      await stopAgent(runningSession.id);
    } catch (e) {
      console.error('Failed to stop agent:', e);
    }
  }, [runningSession, stopAgent]);

  const canSend = !!project && !!selectedAgent && prompt.trim() && !runningSession;

  const handlePromptChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    const value = e.target.value;
    setPrompt(value);
    const el = e.target;
    el.style.height = 'auto';
    el.style.height = Math.min(el.scrollHeight, 180) + 'px';
  };

  // Handle trigger characters
  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    const textarea = e.currentTarget;
    const text = textarea.value;
    const cursorPos = textarea.selectionStart;

    // Ctrl+Enter to send
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && canSend) {
      handleSend();
      return;
    }

    // Trigger characters — only when at start of input or after whitespace
    if (e.key === '@' || e.key === '/' || e.key === '$') {
      const beforeChar = cursorPos === 0 ? ' ' : text[cursorPos - 1];
      if (beforeChar === ' ' || beforeChar === '\n' || cursorPos === 0) {
        // Calculate position for the trigger menu
        // Use a simple approach — position at the composer bottom
        setTriggerMenu({
          type: e.key as '@' | '/' | '$',
          position: { top: 0, left: 0 },
        });
      }
    }

    // Escape to close trigger menu
    if (e.key === 'Escape' && triggerMenu) {
      setTriggerMenu(null);
    }
  };

  const handleTriggerSelect = (item: { label: string; path?: string }) => {
    setTriggerMenu(null);
    if (triggerMenu?.type === '@') {
      // Insert file reference
      const file: AttachedFile = { path: item.path || item.label, name: item.label };
      if (!attachedFiles.some(f => f.path === file.path)) {
        setAttachedFiles(prev => [...prev, file]);
      }
      // Remove the @ from prompt if it was just typed
      if (prompt.endsWith('@')) {
        setPrompt(prev => prev.slice(0, -1));
      }
    } else if (triggerMenu?.type === '/') {
      // Insert command
      setPrompt(prev => {
        const trimmed = prev.endsWith('/') ? prev.slice(0, -1) : prev;
        return trimmed + item.label + ' ';
      });
    } else if (triggerMenu?.type === '$') {
      // Insert skill template
      setPrompt(prev => {
        const trimmed = prev.endsWith('$') ? prev.slice(0, -1) : prev;
        return trimmed + `[${item.label}] `;
      });
    }
    textareaRef.current?.focus();
  };

  const removeAttachedFile = (path: string) => {
    setAttachedFiles(prev => prev.filter(f => f.path !== path));
  };

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

  const selectedAgentInfo = agents.find(a => a.agentType === selectedAgent);
  const selectedAgentLabel = selectedAgentInfo?.displayName || (selectedAgent ? String(selectedAgent) : '选择 Agent');

  const hasConversation = !!(activeSessionId || runningSession);

  // Dynamic placeholder
  const placeholderText = isContinuing
    ? '继续对话... @ 文件 / 命令 $ 技能'
    : hasConversation
      ? '输入指令... @ 文件 / 命令 $ 技能'
      : '输入需求或指令开始新对话... @ 文件 / 命令 $ 技能';

  if (!project) {
    return (
      <div className="agent-panel">
        <div className="agent-panel-landing">
          <div className="agent-panel-landing-icon">DW</div>
          <h2 className="agent-panel-landing-title">Dev Workbench</h2>
          <p className="agent-panel-landing-desc">从左侧选择项目开始工作</p>
        </div>
      </div>
    );
  }

  return (
    <div className="agent-panel">
      {/* Header */}
      <div className="agent-panel-header">
        <div className="agent-panel-header-left">
          <span className="agent-panel-header-project">{project.name}</span>
          {hasConversation && (
            <>
              <span className="agent-panel-header-sep">›</span>
              <span className="agent-panel-header-title">
                {activeRequirement?.title
                  || displaySession?.prompt?.slice(0, 40)
                  || '新对话'}
              </span>
            </>
          )}
        </div>
        <div className="agent-panel-header-right">
          {runningSession ? (
            <>
              <span className="agent-workspace-running">
                <span className="agent-workspace-running-dot" />
                {elapsed}
              </span>
              <button className="agent-workspace-stop-btn" onClick={handleStop}>
                <IconStop size={12} /> 停止
              </button>
            </>
          ) : hasConversation ? (
            <>
              <QualityBadge report={qualityReport} />
              <span className="agent-workspace-idle">
                {displaySession?.status === 'completed' ? '已完成' : displaySession?.status === 'failed' ? '失败' : '就绪'}
              </span>
            </>
          ) : null}
        </div>
      </div>

      {/* Body */}
      <div className="agent-panel-body">
        {!hasConversation && !runningSession ? (
          <div className="agent-panel-empty">
            <div className="agent-panel-empty-icon">💬</div>
            <h2>开始对话</h2>
            <p>在下方输入指令开始，或从左侧选择已有对话</p>
          </div>
        ) : (
          <>
            {activeRequirement && !displaySession && (
              <div className="agent-panel-spec">
                <div className="agent-panel-spec-title">{activeRequirement.title}</div>
                {activeRequirement.description && (
                  <div className="agent-panel-spec-desc">{activeRequirement.description}</div>
                )}
                <div className="agent-panel-spec-actions">
                  <button
                    className="spec-item-launch-btn"
                    onClick={async () => {
                      const agent = selectedAgent || installedAgents[0]?.agentType;
                      if (agent && project) {
                        const session = await launchForRequirement(project.path, activeRequirement.id, agent);
                        if (session) selectSession(session.id);
                      }
                    }}
                    disabled={installedAgents.length === 0}
                  >
                    ▶ 启动 Agent
                  </button>
                  <button
                    className="spec-item-delete-btn"
                    onClick={() => deleteRequirement(activeRequirement.id)}
                  >
                    删除
                  </button>
                </div>
              </div>
            )}
            <div className="agent-workspace-output">
              <TerminalView
                sessionId={runningSession?.id ?? (displaySession?.status === 'running' ? displaySession.id : null)}
                completedSession={!runningSession && displaySession ? displaySession : null}
              />
            </div>
            {qualityReport && (
              <QualityReportPanel report={qualityReport} />
            )}
          </>
        )}
      </div>

      {/* Composer */}
      <div className="agent-workspace-composer" style={{ position: 'relative' }}>
        {/* Trigger menu overlay */}
        {triggerMenu && (
          <TriggerMenu
            type={triggerMenu.type}
            position={triggerMenu.position}
            onSelect={handleTriggerSelect}
            onClose={() => setTriggerMenu(null)}
          />
        )}

        {/* Agent selector */}
        <div className="agent-workspace-composer-agent">
          <div className="agent-workspace-selector" onClick={() => setShowAgentDropdown(!showAgentDropdown)}>
            <span>{selectedAgentLabel}</span>
            {recommended === selectedAgent && <span className="agent-workspace-rec-badge">推荐</span>}
            <span className="agent-workspace-selector-arrow">▾</span>
          </div>
          {showAgentDropdown && (
            <div className="agent-workspace-dropdown">
              {installedAgents.map(agent => (
                <button
                  key={agent.agentType}
                  className={`agent-workspace-dropdown-item ${selectedAgent === agent.agentType ? 'active' : ''}`}
                  onClick={() => { setSelectedAgent(agent.agentType); setShowAgentDropdown(false); }}
                >
                  {agent.displayName}
                  {recommended === agent.agentType && <span className="agent-workspace-rec-badge">推荐</span>}
                </button>
              ))}
            </div>
          )}
        </div>

        {/* Attached files chips */}
        {attachedFiles.length > 0 && (
          <div className="file-chips">
            {attachedFiles.map(file => (
              <span key={file.path} className="file-chip">
                @{file.name}
                <button className="file-chip-remove" onClick={() => removeAttachedFile(file.path)}>×</button>
              </span>
            ))}
          </div>
        )}

        {/* Input row with attach button */}
        <div style={{ display: 'flex', gap: 4, alignItems: 'flex-end' }}>
          <button
            className="composer-attach-btn"
            title="附加文件"
            onClick={() => setTriggerMenu({ type: '@', position: { top: 0, left: 0 } })}
          >
            +
          </button>
          <textarea
            ref={textareaRef}
            className="agent-workspace-composer-input"
            placeholder={placeholderText}
            value={prompt}
            onChange={handlePromptChange}
            maxLength={10000}
            onKeyDown={handleKeyDown}
            disabled={!!runningSession}
            rows={1}
          />
        </div>

        {/* Controls: Mode + Model + Send */}
        <div className="composer-controls">
          <ModeSelector value={agentMode} onChange={setAgentMode} />
          <ModelSelector value={selectedModel} onChange={setSelectedModel} />
          <div style={{ flex: 1 }} />
          <span className="agent-workspace-composer-hint">Ctrl+Enter</span>
          {runningSession ? (
            <button className="agent-workspace-composer-btn stop" onClick={handleStop}>
              <IconStop size={14} /> 停止
            </button>
          ) : (
            <button
              className="agent-workspace-composer-btn send"
              onClick={handleSend}
              disabled={!canSend}
            >
              <IconPlay size={14} /> {isContinuing ? '继续' : '发送'}
            </button>
          )}
        </div>
      </div>
    </div>
  );
}
