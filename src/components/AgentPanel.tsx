import { useState, useCallback, useMemo } from 'react';
import type { Project, Session, Requirement, AgentInfo, AgentType } from '../types';
import { AgentSelector } from './AgentSelector';
import { AgentOutput } from './AgentOutput';
import { RequirementList } from './RequirementList';
import { SessionTimeline } from './SessionTimeline';
import { IconX, IconPlay, IconStop, IconHistory, IconSparkles } from './Icons';

interface AgentPanelProps {
  project: Project;
  sessions: Session[];
  requirements: Requirement[];
  agents: AgentInfo[];
  onClose: () => void;
  spawnAgent: (projectPath: string, agentType: AgentType, prompt: string, model?: string, linkedRequirementId?: string) => Promise<Session>;
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
  const [recommended, setRecommended] = useState<AgentType | null>(null);

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
      await spawnAgent(project.path, selectedAgent, prompt.trim());
      setPrompt('');
    } catch (e) {
      console.error('Failed to spawn agent:', e);
    }
  }, [selectedAgent, prompt, runningSession, project.path, spawnAgent]);

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
    await updateRequirement(id, { status: 'in_progress' });
  }, [updateRequirement]);

  const handleMarkDone = useCallback(async (id: string) => {
    await updateRequirement(id, { status: 'done' });
  }, [updateRequirement]);

  const handleContinueWith = useCallback(async (session: Session, targetAgent: AgentType) => {
    const truncated = session.prompt.length > 5000 ? session.prompt.slice(0, 5000) + '...' : session.prompt;
    const newPrompt = `[继续 session ${session.id}]\n${truncated}`;
    await spawnAgent(project.path, targetAgent, newPrompt);
  }, [project.path, spawnAgent]);

  const canSend = selectedAgent && prompt.trim() && !runningSession;

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

        <div className="agent-panel-content">
          {activeTab === 'active' && (
            <div className="agent-panel-active">
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

              <AgentOutput sessionId={runningSession?.id ?? null} />
            </div>
          )}

          {activeTab === 'requirements' && (
            <RequirementList
              requirements={projectRequirements}
              sessions={projectSessions}
              projectPath={project.path}
              onAdd={handleAddRequirement}
              onStart={handleStartRequirement}
              onMarkDone={handleMarkDone}
            />
          )}

          {activeTab === 'history' && (
            <SessionTimeline
              sessions={projectSessions}
              onContinueWith={handleContinueWith}
            />
          )}
        </div>
      </div>
    </div>
  );
}
