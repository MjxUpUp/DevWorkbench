import { create } from 'zustand';
import { useActivityStore } from './activityStore';
import { useNavigationStore } from './navigationStore';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { AgentInfo, Session, Requirement, AgentType, QualityReport } from '../types';

interface AgentState {
  agents: AgentInfo[];
  sessions: Session[];
  requirements: Requirement[];
  loading: boolean;
  ptyOutput: Map<string, Uint8Array[]>;
  qualityReports: Map<string, QualityReport>;

  refreshAgents: () => Promise<void>;
  refreshSessions: () => Promise<void>;
  refreshRequirements: () => Promise<void>;
  spawnAgent: (
    projectPath: string,
    agentType: AgentType,
    prompt: string,
    model?: string,
    linkedRequirementId?: string,
    parentSessionId?: string,
  ) => Promise<Session>;
  stopAgent: (sessionId: string) => Promise<void>;
  addRequirement: (req: Requirement) => Promise<Requirement[]>;
  updateRequirement: (id: string, patch: Record<string, unknown>) => Promise<Requirement[]>;
  removeRequirement: (id: string) => Promise<Requirement[]>;
  getSessionsForProject: (projectPath: string) => Session[];
  getRequirementsForProject: (projectPath: string) => Requirement[];
  recommendAgent: (tags: string[]) => Promise<AgentType | null>;
  fetchQualityReport: (sessionId: string) => Promise<QualityReport | null>;
  getQualityReport: (sessionId: string) => QualityReport | null;
  newConversation: (projectPath: string, title: string, agentType: AgentType) => Promise<Session>;
  launchForRequirement: (projectPath: string, requirementId: string, agentType: string) => Promise<Session | null>;
  getDefaultAgent: () => AgentType | null;
  appendPtyOutput: (sessionId: string, data: Uint8Array) => void;
  clearPtyOutput: (sessionId: string) => void;
  initEventListeners: () => () => void;
}

export const useAgentStore = create<AgentState>((set, get) => ({
  agents: [],
  sessions: [],
  requirements: [],
  loading: true,
  ptyOutput: new Map(),
  qualityReports: new Map(),

  refreshAgents: async () => {
    try {
      const result = await invoke<AgentInfo[]>('discover_agents_cmd');
      set({ agents: result });
    } catch (e) {
      console.error('Failed to discover agents:', e);
    }
  },

  refreshSessions: async () => {
    try {
      const result = await invoke<Session[]>('load_sessions');
      set({ sessions: result });
    } catch (e) {
      console.error('Failed to load sessions:', e);
    }
  },

  refreshRequirements: async () => {
    try {
      const result = await invoke<Requirement[]>('load_requirements');
      set({ requirements: result });
    } catch (e) {
      console.error('Failed to load requirements:', e);
    }
  },

  spawnAgent: async (projectPath, agentType, prompt, model, linkedRequirementId, parentSessionId) => {
    const session = await invoke<Session>('spawn_agent_session', {
      projectPath,
      agentType,
      prompt,
      model: model || null,
      linkedRequirementId: linkedRequirementId || null,
      parentSessionId: parentSessionId || null,
    });
    set((s) => ({ sessions: [...s.sessions, session] }));
    return session;
  },

  stopAgent: async (sessionId) => {
    await invoke('stop_agent_session', { sessionId });
    await get().refreshSessions();
  },

  addRequirement: async (req) => {
    await invoke('add_requirement', { req });
    await get().refreshRequirements();
    return get().requirements;
  },

  updateRequirement: async (id, patch) => {
    await invoke('update_requirement', { id, patch });
    await get().refreshRequirements();
    return get().requirements;
  },

  removeRequirement: async (id) => {
    await invoke('remove_requirement', { id });
    await get().refreshRequirements();
    return get().requirements;
  },

  getSessionsForProject: (projectPath) => {
    return get().sessions.filter((s) => s.projectPath === projectPath);
  },

  getRequirementsForProject: (projectPath) => {
    return get().requirements.filter((r) => r.projectPath === projectPath);
  },

  recommendAgent: async (tags) => {
    return invoke<AgentType | null>('recommend_agent_for_project', { tags });
  },

  fetchQualityReport: async (sessionId) => {
    try {
      const report = await invoke<QualityReport | null>('get_quality_report_for_session', { sessionId });
      if (report) {
        set((s) => {
          const next = new Map(s.qualityReports);
          next.set(sessionId, report);
          return { qualityReports: next };
        });
      }
      return report;
    } catch (e) {
      console.error('Failed to fetch quality report:', e);
      return null;
    }
  },

  getQualityReport: (sessionId) => {
    return get().qualityReports.get(sessionId) ?? null;
  },

  newConversation: async (projectPath, title, agentType) => {
    // 插入 requirement 前先校验：必须有可用的 agent，否则 spawn 必失败、
    // requirement 会变成 in_progress 的孤儿（spawn 卡死时 catch 不触发）。
    if (!agentType) {
      throw new Error('没有可用的 Agent：请先在设置中确认 CLI 已安装');
    }
    const reqId = crypto.randomUUID();
    const newReq: Requirement = {
      id: reqId,
      projectPath,
      title,
      description: null,
      status: 'in_progress',
      priority: null,
      linkedSessionId: null,
      artifacts: [],
      createdAt: new Date().toISOString(),
      updatedAt: new Date().toISOString(),
    };
    await get().addRequirement(newReq);
    try {
      const session = await get().spawnAgent(projectPath, agentType, title, undefined, reqId, undefined);
      await get().updateRequirement(reqId, {
        linkedSessionId: session.id,
        updatedAt: new Date().toISOString(),
      });
      return session;
    } catch (e) {
      console.error('Failed to spawn agent:', e);
      // spawn 失败/卡死 → 回滚 requirement，避免留下 in_progress 孤儿
      await get().updateRequirement(reqId, {
        status: 'todo',
        linkedSessionId: null,
        updatedAt: new Date().toISOString(),
      });
      throw e;
    }
  },

  launchForRequirement: async (projectPath, requirementId, agentType) => {
    const reqs = get().getRequirementsForProject(projectPath);
    const req = reqs.find(r => r.id === requirementId);
    if (!req) return null;
    try {
      const session = await get().spawnAgent(projectPath, agentType as AgentType, req.title, undefined, requirementId, undefined);
      await get().updateRequirement(requirementId, {
        status: 'in_progress',
        linkedSessionId: session.id,
        updatedAt: new Date().toISOString(),
      });
      return session;
    } catch (e) {
      console.error('Failed to spawn agent for requirement:', e);
      return null;
    }
  },

  getDefaultAgent: () => {
    const installed = get().agents.filter(a => a.installed);
    return installed.length > 0 ? installed[0].agentType : null;
  },

  appendPtyOutput: (sessionId, data) => {
    set((s) => {
      const next = new Map(s.ptyOutput);
      const existing = next.get(sessionId) || [];
      next.set(sessionId, [...existing, data]);
      return { ptyOutput: next };
    });
  },

  clearPtyOutput: (sessionId) => {
    set((s) => {
      const next = new Map(s.ptyOutput);
      next.delete(sessionId);
      return { ptyOutput: next };
    });
  },

  initEventListeners: () => {
    const { refreshSessions, refreshRequirements } = get();

    // Store all unlisten functions — wait for promises to resolve
    const unlisteners: Array<() => void> = [];
    let cancelled = false;

    const p1 = listen('agent:started', () => {
      refreshSessions();
    }).then((fn) => { if (!cancelled) unlisteners.push(fn); else fn(); });

    const p2 = listen<{ sessionId: string; status: string; exitCode: number | null }>('agent:completed', async (event) => {
      const { sessionId: completedId, status: completedStatus, exitCode: completedExitCode } = event.payload;

      // Immediately update the session in store from event payload — don't wait for DB round-trip.
      // This prevents the UI from showing "running" when the DB read returns stale WAL data.
      set((s) => ({
        sessions: s.sessions.map((ses) =>
          ses.id === completedId
            ? {
                ...ses,
                status: completedStatus === 'completed' ? 'completed' : 'failed',
                exitCode: completedExitCode ?? ses.exitCode,
                finishedAt: new Date().toISOString(),
              }
            : ses
        ),
      }));

      // Full sync from DB in background (picks up outputSummary, contextSnapshot, etc.)
      get().refreshSessions();
      get().refreshRequirements();

      // Refresh activity timeline so new events show up immediately
      const { loadForProject, loadRecent } = useActivityStore.getState();
      const activeProject = useNavigationStore.getState().activeProject;
      if (activeProject) {
        await loadForProject(activeProject.path);
      } else {
        await loadRecent(100);
      }

      const sessionId = event.payload.sessionId;
      // Auto-fetch quality report for completed sessions
      get().fetchQualityReport(sessionId);
      const sessions = get().sessions;
      const session = sessions.find((s) => s.id === sessionId);
      if (session?.linkedRequirementId && session.status === 'completed') {
        await invoke('update_requirement', {
          id: session.linkedRequirementId,
          patch: { status: 'done', updatedAt: new Date().toISOString() },
        });
        await refreshRequirements();
      }
    }).then((fn) => { if (!cancelled) unlisteners.push(fn); else fn(); });

    const p3 = listen<{ sessionId: string; data: number[] }>('pty:output', (event) => {
      get().appendPtyOutput(event.payload.sessionId, new Uint8Array(event.payload.data));
    }).then((fn) => { if (!cancelled) unlisteners.push(fn); else fn(); });

    // Initial load
    Promise.all([get().refreshAgents(), refreshSessions(), refreshRequirements()])
      .finally(() => set({ loading: false }));

    return () => {
      cancelled = true;
      // If promises already resolved, unlisteners are populated; if not, the .then()
      // callbacks will call fn() immediately to clean up.
      unlisteners.forEach((fn) => fn());
      // Also await pending promises to ensure no listeners leak
      Promise.all([p1, p2, p3]).catch(() => {});
    };
  },
}));
