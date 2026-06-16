import { create } from 'zustand';
import { useActivityStore } from './activityStore';
import { useNavigationStore } from './navigationStore';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { AgentInfo, Session, AgentType, Conversation, QualityReport, ChatStreamEvent } from '../types';

interface AgentState {
  agents: AgentInfo[];
  /** All turns (sessions) across every project. A turn is one user prompt →
   *  one agent run; turns group into conversations via `conversationId`. */
  sessions: Session[];
  /** All conversation containers, across every project. */
  conversations: Conversation[];
  loading: boolean;
  ptyOutput: Map<string, Uint8Array[]>;
  /** Live in-memory structured blocks per session, accumulated from the
   *  `agent:event` channel while a session runs. Cleared on agent:completed —
   *  finalized sessions replay from the persisted `session.blocks` column. */
  sessionBlocks: Map<string, ChatStreamEvent[]>;
  qualityReports: Map<string, QualityReport>;

  refreshAgents: () => Promise<void>;
  refreshSessions: () => Promise<void>;
  refreshConversations: (projectPath: string) => Promise<void>;
  spawnAgent: (
    projectPath: string,
    agentType: AgentType,
    prompt: string,
    model?: string,
    linkedRequirementId?: string,
    parentSessionId?: string,
    conversationId?: string,
    kernel?: boolean,
  ) => Promise<Session>;
  stopAgent: (sessionId: string) => Promise<void>;
  getSessionsForProject: (projectPath: string) => Session[];
  /** Turns of one conversation, oldest-first. */
  getTurnsForConversation: (conversationId: string) => Session[];
  /** Conversations belonging to a project, newest-activity-first. */
  getConversationsForProject: (projectPath: string) => Conversation[];
  /** Resolve the conversation a turn belongs to (for activity→conversation jumps). */
  getConversationForSession: (sessionId: string) => Conversation | null;
  updateConversation: (id: string, patch: Record<string, unknown>) => Promise<void>;
  recommendAgent: (tags: string[]) => Promise<AgentType | null>;
  fetchQualityReport: (sessionId: string) => Promise<QualityReport | null>;
  getQualityReport: (sessionId: string) => QualityReport | null;
  /** First turn of a brand-new conversation (no conversation_id yet). */
  createConversation: (projectPath: string, prompt: string, agentType: AgentType, kernel?: boolean) => Promise<Session>;
  /** Append a follow-up turn to an existing conversation. The agent may differ
   *  from prior turns — that's the whole point of the conversation container. */
  continueConversation: (
    projectPath: string,
    conversationId: string,
    prompt: string,
    agentType: AgentType,
    kernel?: boolean,
  ) => Promise<Session>;
  getDefaultAgent: () => AgentType | null;
  appendPtyOutput: (sessionId: string, data: Uint8Array) => void;
  clearPtyOutput: (sessionId: string) => void;
  appendBlock: (sessionId: string, event: ChatStreamEvent) => void;
  clearBlocks: (sessionId: string) => void;
  initEventListeners: () => () => void;
}

export const useAgentStore = create<AgentState>((set, get) => ({
  agents: [],
  sessions: [],
  conversations: [],
  loading: true,
  ptyOutput: new Map(),
  sessionBlocks: new Map(),
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
      set((s) => {
        // Merge instead of replace. load_sessions reads from SQLite, and a
        // just-spawned running session may not be visible to that read yet
        // (WAL stale snapshot / write not committed at the instant the
        // `agent:started` event fires refreshSessions). A blanket
        // `set({ sessions: result })` wiped that in-memory running session —
        // which is why switching projects and back mid-run showed an empty
        // view, and why history flickered out after any event-driven refresh.
        // DB result stays authoritative for sessions it knows; in-memory-only
        // sessions are preserved and reconcile on the next refresh.
        const dbIds = new Set(result.map((r) => r.id));
        const localOnly = s.sessions.filter((sess) => !dbIds.has(sess.id));
        return { sessions: [...result, ...localOnly] };
      });
    } catch (e) {
      console.error('Failed to load sessions:', e);
    }
  },

  refreshConversations: async (projectPath) => {
    try {
      const result = await invoke<Conversation[]>('list_conversations', { projectPath });
      set((s) => {
        // Same merge rationale as refreshSessions: a just-spawned turn may
        // have created a conversation the backend list_conversations read
        // doesn't surface yet (WAL lag). Preserve local-only conversations.
        const dbIds = new Set(result.map((r) => r.id));
        const localOnly = s.conversations.filter((c) => !dbIds.has(c.id));
        return { conversations: [...result, ...localOnly] };
      });
    } catch (e) {
      console.error('Failed to load conversations:', e);
    }
  },

  spawnAgent: async (projectPath, agentType, prompt, model, linkedRequirementId, parentSessionId, conversationId, kernel) => {
    const session = await invoke<Session>('spawn_agent_session', {
      projectPath,
      agentType,
      prompt,
      model: model || null,
      linkedRequirementId: linkedRequirementId || null,
      parentSessionId: parentSessionId || null,
      conversationId: conversationId || null,
      kernel: kernel ?? false,
    });
    set((s) => ({ sessions: [...s.sessions, session] }));
    // If this turn created/attached a conversation, refresh that project's
    // conversation list so the sidebar shows it. Derive the project from the
    // turn we just spawned (covers both create + continue).
    void get().refreshConversations(session.projectPath);
    return session;
  },

  stopAgent: async (sessionId) => {
    await invoke('stop_agent_session', { sessionId });
    await get().refreshSessions();
  },

  getSessionsForProject: (projectPath) => {
    return get().sessions.filter((s) => s.projectPath === projectPath);
  },

  getTurnsForConversation: (conversationId) => {
    return get()
      .sessions.filter((s) => s.conversationId === conversationId)
      .sort((a, b) => new Date(a.startedAt).getTime() - new Date(b.startedAt).getTime());
  },

  getConversationsForProject: (projectPath) => {
    // pinned first, then most-recent activity. Mirrors the backend's
    // load_conversations_for_project_db ORDER BY.
    return get()
      .conversations
      .filter((c) => c.projectPath === projectPath)
      .sort((a, b) => {
        if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
        return new Date(b.lastActivityAt).getTime() - new Date(a.lastActivityAt).getTime();
      });
  },

  getConversationForSession: (sessionId) => {
    const session = get().sessions.find((s) => s.id === sessionId);
    if (!session?.conversationId) return null;
    return get().conversations.find((c) => c.id === session.conversationId) ?? null;
  },

  updateConversation: async (id, patch) => {
    await invoke('update_conversation', { id, patch });
    // Optimistically mirror into local state; the next refreshConversations reconciles.
    set((s) => ({
      conversations: s.conversations.map((c) => (c.id === id ? { ...c, ...patch } as Conversation : c)),
    }));
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

  createConversation: async (projectPath, prompt, agentType, kernel) => {
    if (!agentType) {
      throw new Error('没有可用的 Agent：请先在设置中确认 CLI 已安装');
    }
    // No conversation_id → backend creates a new container and attaches this
    // turn as its first. The returned session carries the new conversationId.
    return get().spawnAgent(projectPath, agentType, prompt, undefined, undefined, undefined, undefined, kernel);
  },

  continueConversation: async (projectPath, conversationId, prompt, agentType, kernel) => {
    if (!agentType) {
      throw new Error('没有可用的 Agent：请先在设置中确认 CLI 已安装');
    }
    // conversation_id present → backend attaches this as a follow-up turn of
    // the existing container and touches its last_agent / last_activity_at.
    return get().spawnAgent(projectPath, agentType, prompt, undefined, undefined, undefined, conversationId, kernel);
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

  appendBlock: (sessionId, event) => {
    set((s) => {
      const next = new Map(s.sessionBlocks);
      const existing = next.get(sessionId) ?? [];
      // Merge consecutive text deltas into one block. Real streaming emits a
      // Text event per token; BlocksView renders each event as its own card, so
      // without merging a long reply explodes into hundreds of cards and shatters
      // Markdown across block boundaries. Concatenate onto the last text block.
      const last = existing[existing.length - 1];
      if (event.kind === 'text' && last && last.kind === 'text') {
        next.set(sessionId, [
          ...existing.slice(0, -1),
          { kind: 'text', content: last.content + event.content },
        ]);
      } else {
        next.set(sessionId, [...existing, event]);
      }
      return { sessionBlocks: next };
    });
  },

  clearBlocks: (sessionId) => {
    set((s) => {
      const next = new Map(s.sessionBlocks);
      next.delete(sessionId);
      return { sessionBlocks: next };
    });
  },

  initEventListeners: () => {
    const { refreshSessions } = get();

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

      // Full sync from DB in background (picks up outputSummary, contextSnapshot,
      // and the persisted blocks column so the completed turn replays via
      // BlocksView after a reload/project-switch).
      get().refreshSessions();
      // Drop the live in-memory blocks now that the session is finalized — the
      // persisted session.blocks (read back above) is the source of truth for a
      // completed turn, and a stale live snapshot would shadow it on re-render.
      get().clearBlocks(completedId);
      // A turn completing means its conversation's last_activity_at moved;
      // refresh that project's conversation list so the sidebar reorders.
      const doneSession = get().sessions.find((s) => s.id === completedId);
      if (doneSession) {
        void get().refreshConversations(doneSession.projectPath);
      }

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
    }).then((fn) => { if (!cancelled) unlisteners.push(fn); else fn(); });

    const p3 = listen<{ sessionId: string; data: number[] }>('pty:output', (event) => {
      get().appendPtyOutput(event.payload.sessionId, new Uint8Array(event.payload.data));
    }).then((fn) => { if (!cancelled) unlisteners.push(fn); else fn(); });

    // Structured agent output — one ChatStreamEvent per parsed block of the
    // agent's stream (claude stream-json today; ReactAgent later). The chat UI
    // folds these into block cards via BlocksView. Raw agents (pi) emit no
    // agent:event, so they keep the TerminalView/Markdown path unchanged.
    const p4 = listen<{ sessionId: string; event: ChatStreamEvent }>('agent:event', (event) => {
      get().appendBlock(event.payload.sessionId, event.payload.event);
    }).then((fn) => { if (!cancelled) unlisteners.push(fn); else fn(); });

    // Initial load
    Promise.all([get().refreshAgents(), refreshSessions()])
      .finally(() => set({ loading: false }));

    return () => {
      cancelled = true;
      // If promises already resolved, unlisteners are populated; if not, the .then()
      // callbacks will call fn() immediately to clean up.
      unlisteners.forEach((fn) => fn());
      // Also await pending promises to ensure no listeners leak
      Promise.all([p1, p2, p3, p4]).catch(() => {});
    };
  },
}));
