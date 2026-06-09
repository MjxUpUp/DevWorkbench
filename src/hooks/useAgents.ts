import { useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { AgentInfo, Session, Requirement, AgentType } from '../types';

export function useAgents() {
  const [agents, setAgents] = useState<AgentInfo[]>([]);
  const [sessions, setSessions] = useState<Session[]>([]);
  const [requirements, setRequirements] = useState<Requirement[]>([]);
  const [loading, setLoading] = useState(true);

  const refreshAgents = useCallback(async () => {
    try {
      const result = await invoke<AgentInfo[]>('discover_agents_cmd');
      setAgents(result);
    } catch (e) {
      console.error('Failed to discover agents:', e);
    }
  }, []);

  const refreshSessions = useCallback(async () => {
    try {
      const result = await invoke<Session[]>('load_sessions');
      setSessions(result);
    } catch (e) {
      console.error('Failed to load sessions:', e);
    }
  }, []);

  const refreshRequirements = useCallback(async () => {
    try {
      const result = await invoke<Requirement[]>('load_requirements');
      setRequirements(result);
    } catch (e) {
      console.error('Failed to load requirements:', e);
    }
  }, []);

  // Initial load
  useEffect(() => {
    Promise.all([refreshAgents(), refreshSessions(), refreshRequirements()])
      .finally(() => setLoading(false));
  }, []);

  // Listen for agent events
  useEffect(() => {
    const unlistenStarted = listen('agent:started', () => {
      refreshSessions();
    });
    const unlistenCompleted = listen('agent:completed', () => {
      refreshSessions();
      refreshRequirements();
    });

    return () => {
      unlistenStarted.then(fn => fn());
      unlistenCompleted.then(fn => fn());
    };
  }, [refreshSessions, refreshRequirements]);

  // Spawn agent
  const spawnAgent = useCallback(async (
    projectPath: string,
    agentType: AgentType,
    prompt: string,
    model?: string,
    linkedRequirementId?: string,
    parentSessionId?: string,
  ) => {
    const session = await invoke<Session>('spawn_agent_session', {
      projectPath,
      agentType,
      prompt,
      model: model || null,
      linkedRequirementId: linkedRequirementId || null,
      parentSessionId: parentSessionId || null,
    });
    setSessions(prev => [...prev, session]);
    return session;
  }, []);

  // Stop agent
  const stopAgent = useCallback(async (sessionId: string) => {
    await invoke('stop_agent_session', { sessionId });
    refreshSessions();
  }, [refreshSessions]);

  // Add requirement
  const addRequirement = useCallback(async (req: Requirement) => {
    const result = await invoke<Requirement[]>('add_requirement', { req });
    setRequirements(result);
    return result;
  }, []);

  // Update requirement
  const updateRequirement = useCallback(async (id: string, patch: Record<string, unknown>) => {
    const result = await invoke<Requirement[]>('update_requirement', { id, patch });
    setRequirements(result);
    return result;
  }, []);

  // Remove requirement
  const removeRequirement = useCallback(async (id: string) => {
    const result = await invoke<Requirement[]>('remove_requirement', { id });
    setRequirements(result);
    return result;
  }, []);

  const getSessionsForProject = useCallback((projectPath: string) => {
    return sessions.filter(s => s.projectPath === projectPath);
  }, [sessions]);

  const getRequirementsForProject = useCallback((projectPath: string) => {
    return requirements.filter(r => r.projectPath === projectPath);
  }, [requirements]);

  const runningSessions = sessions.filter(s => s.status === 'running');
  const installedAgents = agents.filter(a => a.installed);

  const recommendAgent = useCallback(async (tags: string[]) => {
    return invoke<AgentType | null>('recommend_agent_for_project', { tags });
  }, []);

  return {
    agents,
    installedAgents,
    sessions,
    requirements,
    runningSessions,
    loading,
    refreshAgents,
    refreshSessions,
    refreshRequirements,
    spawnAgent,
    stopAgent,
    addRequirement,
    updateRequirement,
    removeRequirement,
    getSessionsForProject,
    getRequirementsForProject,
    recommendAgent,
  };
}
