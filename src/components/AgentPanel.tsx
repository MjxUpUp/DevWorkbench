import { useState, useCallback, useMemo, useRef, useEffect } from 'react';
import type { Project, Session, Requirement, AgentInfo, AgentType } from '../types';
import { AgentSelector } from './AgentSelector';
import { TerminalView } from './TerminalView';
import { RequirementList } from './RequirementList';
import { SessionTimeline } from './SessionTimeline';
import { IconX, IconPlay, IconStop, IconHistory, IconSparkles } from './Icons';
import { useToast } from './Toast';

interface AgentPanelProps {
  project: Project;
  sessions: Session[];
  requirements: Requirement[];
  agents: AgentInfo[];
  onClose: () => void;
  spawnAgent: (projectPath: string, agentType: AgentType, prompt: string, model?: string, linkedRequirementId?: string, parentSessionId?: string) => Promise<Session>;
  stopAgent: (sessionId: string) => Promise<void>;
  addRequirement: (req: Requirement) => Promise<Requirement[]>;
  updateRequirement: (id: string, patch: Record<string, unknown>) => Promise<Requirement[]>;
  getSessionsForProject: (projectPath: string) => Session[];
  getRequirementsForProject: (projectPath: string) => Requirement[];
  recommendAgent: (tags: string[]) => Promise<AgentType | null>;
}

type PanelTab = 'active' | 'requirements' | 'history';

export function AgentPanel({
  project,
  sessions: allSessions,
  requirements: allRequirements,
  agents,
  onClose,
  spawnAgent,
  stopAgent,
  addRequirement,
  updateRequirement,
  getSessionsForProject,
  getRequirementsForProject,
  recommendAgent,
}: AgentPanelProps) {
  const [activeTab, setActiveTab] = useState<PanelTab>('active');
  const [selectedAgent, setSelectedAgent] = useState<AgentType | null>(null);
  const [prompt, setPrompt] = useState('');
  const [continueFromId, setContinueFromId] = useState<string | null>(null);
  const [recommended, setRecommended] = useState<AgentType | null>(null);
  const toast = useToast();
  const [splitRatio, setSplitRatio] = useState(() => {
    try {
      const saved = localStorage.getItem(`agent-panel-split:${project.path}`);
      return saved ? parseFloat(saved) : 0.35;
    } catch { return 0.35; }
  });
  const [isDragging, setIsDragging] = useState(false);
  const contentRef = useRef<HTMLDivElement>(null);
  const dragRef = useRef({ startY: 0, startRatio: 0 });
  const splitRatioRef = useRef(splitRatio);
  splitRatioRef.current = splitRatio;

  const projectSessions = useMemo(
    () => getSessionsForProject(project.path),
    [getSessionsForProject, project.path, allSessions]
  );
  const projectRequirements = useMemo(
    () => getRequirementsForProject(project.path),
    [getRequirementsForProject, project.path, allRequirements]
  );

  const runningSession = useMemo(
    () => projectSessions.find(s => s.status === 'running') ?? null,
    [projectSessions]
  );

  const installedAgents = useMemo(
    () => agents.filter(a => a.installed),
    [agents]
  );

  // Auto-select first installed agent or recommended
  useMemo(() => {
    if (selectedAgent) return;
    recommendAgent(project.tags).then(rec => {
      setRecommended(rec);
      if (rec && agents.find(a => a.agentType === rec)?.installed) {
        setSelectedAgent(rec);
      } else if (installedAgents.length > 0) {
        setSelectedAgent(installedAgents[0].agentType);
      }
    });
  }, []);

  const handleSend = useCallback(async () => {
    if (!selectedAgent || !prompt.trim() || runningSession) return;
    try {
      await spawnAgent(project.path, selectedAgent, prompt.trim(), undefined, undefined, continueFromId ?? undefined);
      setPrompt('');
      setContinueFromId(null);
    } catch (e) {
      console.error('Failed to spawn agent:', e);
    }
  }, [selectedAgent, prompt, runningSession, project.path, spawnAgent, continueFromId]);

  const handleStop = useCallback(async () => {
    if (!runningSession) return;
    try {
      await stopAgent(runningSession.id);
    } catch (e) {
      console.error('Failed to stop agent:', e);
    }
  }, [runningSession, stopAgent]);

  const handleAddRequirement = useCallback(async (title: string) => {
    const req: Requirement = {
      id: '',
      projectPath: project.path,
      title,
      description: null,
      status: 'todo',
      priority: null,
      linkedSessionId: null,
      artifacts: [],
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
    };
    await addRequirement(req);
  }, [project.path, addRequirement]);

  const handleStartRequirement = useCallback(async (id: string) => {
    const req = projectRequirements.find(r => r.id === id);
    if (!req) {
      toast.error('需求未找到');
      return;
    }

    // Already running for this project?
    if (runningSession) {
      toast.info('已有 agent 在运行中');
      return;
    }

    // Resolve agent BEFORE updating status — avoids needing to revert on no-agent
    const agent = selectedAgent
      ?? recommended
      ?? (installedAgents.length > 0 ? installedAgents[0].agentType : null);

    if (!agent) {
      toast.error('请先安装 AI 工具');
      return;
    }

    // Now safe to update status
    try {
      await updateRequirement(id, { status: 'in_progress' });
    } catch (e) {
      toast.error('更新需求状态失败');
      return;
    }

    // Spawn agent session
    try {
      const session = await spawnAgent(
        project.path,
        agent,
        req.title,
        undefined,
        req.id,   // linkedRequirementId
      );

      // Link session back to requirement
      await updateRequirement(id, { linkedSessionId: session.id });

      // Switch to Active tab to show terminal output
      setActiveTab('active');
    } catch (e) {
      // Revert status on failure
      await updateRequirement(id, { status: 'todo' }).catch(() => {});
      toast.error(`启动 agent 失败: ${e instanceof Error ? e.message : String(e)}`);
    }
  }, [projectRequirements, runningSession, selectedAgent, recommended, installedAgents, project.path, spawnAgent, updateRequirement, toast]);

  const handleMarkDone = useCallback(async (id: string) => {
    await updateRequirement(id, { status: 'done' });
  }, [updateRequirement]);

  const handleContinueWith = useCallback((session: Session, targetAgent: AgentType) => {
    // Pre-fill prompt with context, switch to Active tab, let user edit and send
    setSelectedAgent(targetAgent);
    setContinueFromId(session.id);
    setActiveTab('active');
    const summary = session.outputSummary
      ? session.outputSummary.length > 200
        ? session.outputSummary.slice(0, 200) + '...'
        : session.outputSummary
      : session.prompt;
    setPrompt(`基于上次对话的上下文继续：\n${summary}\n\n`);
  }, []);

  const canSend = selectedAgent && prompt.trim() && !runningSession;

  // Toggle overflow on .agent-panel-content when Active tab is selected
  useEffect(() => {
    const el = contentRef.current;
    if (el) el.classList.toggle('active-tab', activeTab === 'active');
  }, [activeTab]);

  const handleDividerMouseDown = useCallback((e: React.MouseEvent) => {
    e.preventDefault();
    setIsDragging(true);
    dragRef.current = { startY: e.clientY, startRatio: splitRatioRef.current };
  }, []);

  useEffect(() => {
    if (!isDragging) return;

    let rafId: number | null = null;

    const handleMouseMove = (e: MouseEvent) => {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(() => {
        rafId = null;
        const contentEl = contentRef.current;
        if (!contentEl) return;
        const contentHeight = contentEl.getBoundingClientRect().height;
        if (contentHeight <= 0) return;

        const deltaY = e.clientY - dragRef.current.startY;
        const deltaRatio = deltaY / contentHeight;
        const newRatio = Math.min(0.8, Math.max(0.2, dragRef.current.startRatio + deltaRatio));
        setSplitRatio(newRatio);
      });
    };

    const handleMouseUp = () => {
      setIsDragging(false);
      try {
        localStorage.setItem(`agent-panel-split:${project.path}`, splitRatioRef.current.toString());
      } catch { /* ignore */ }
    };

    document.addEventListener('mousemove', handleMouseMove);
    document.addEventListener('mouseup', handleMouseUp);
    return () => {
      document.removeEventListener('mousemove', handleMouseMove);
      document.removeEventListener('mouseup', handleMouseUp);
      if (rafId !== null) cancelAnimationFrame(rafId);
    };
  }, [isDragging, project.path]);

  return (
    <div className="agent-panel-backdrop" onClick={onClose}>
      <div className="agent-panel" onClick={e => e.stopPropagation()}>
        <div className="agent-panel-header">
          <div className="agent-panel-title">
            <IconSparkles size={16} />
            <span>{project.name}</span>
          </div>
          <button className="agent-panel-close" onClick={onClose}>
            <IconX size={16} />
          </button>
        </div>

        <div className="agent-panel-tabs">
          <button
            className={`agent-panel-tab ${activeTab === 'active' ? 'active' : ''}`}
            onClick={() => setActiveTab('active')}
          >
            Active
          </button>
          <button
            className={`agent-panel-tab ${activeTab === 'requirements' ? 'active' : ''}`}
            onClick={() => setActiveTab('requirements')}
          >
            Requirements {projectRequirements.length > 0 && <span className="agent-panel-tab-count">{projectRequirements.length}</span>}
          </button>
          <button
            className={`agent-panel-tab ${activeTab === 'history' ? 'active' : ''}`}
            onClick={() => setActiveTab('history')}
          >
            <IconHistory size={14} />
            History
          </button>
        </div>

        <div className="agent-panel-content" ref={contentRef}>
          {activeTab === 'active' && (
            <div className="agent-panel-active">
              <div className="agent-split-pane">
                <div className="agent-split-control" style={{ height: `${splitRatio * 100}%` }}>
                  <AgentSelector
                    agents={agents}
                    value={selectedAgent}
                    onChange={setSelectedAgent}
                    recommended={recommended}
                  />

                  <div className="agent-prompt-area">
                    <textarea
                      className="agent-prompt-input"
                      placeholder="描述你想让 agent 做什么..."
                      value={prompt}
                      onChange={e => setPrompt(e.target.value)}
                      maxLength={10000}
                      onKeyDown={e => {
                        if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && canSend) {
                          handleSend();
                        }
                      }}
                      disabled={!!runningSession}
                      rows={3}
                    />
                    <div className="agent-prompt-actions">
                      {runningSession ? (
                        <button className="agent-prompt-btn stop" onClick={handleStop}>
                          <IconStop size={14} />
                          停止
                        </button>
                      ) : (
                        <button className="agent-prompt-btn send" onClick={handleSend} disabled={!canSend}>
                          <IconPlay size={14} />
                          发送
                        </button>
                      )}
                      <span className="agent-prompt-hint">Ctrl+Enter 发送</span>
                    </div>
                  </div>
                </div>

                <div
                  className={`agent-split-divider ${isDragging ? 'dragging' : ''}`}
                  onMouseDown={handleDividerMouseDown}
                />

                <div className="agent-split-terminal" style={{ height: `${(1 - splitRatio) * 100}%` }}>
                  <TerminalView sessionId={runningSession?.id ?? null} />
                </div>
              </div>
            </div>
          )}

          {activeTab === 'requirements' && (
            <RequirementList
              requirements={projectRequirements}
              sessions={projectSessions}
              agents={agents}
              projectPath={project.path}
              onAdd={handleAddRequirement}
              onStart={handleStartRequirement}
              onMarkDone={handleMarkDone}
            />
          )}

          {activeTab === 'history' && (
            <SessionTimeline
              sessions={projectSessions}
              agents={agents}
              onContinueWith={handleContinueWith}
            />
          )}
        </div>
      </div>
    </div>
  );
}
