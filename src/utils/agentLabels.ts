import type { AgentType, AgentInfo } from '../types';

/**
 * Derive a label map from agent discovery data.
 * Falls back to agentType string if display_name is not available.
 */
export function buildAgentLabelMap(agents: AgentInfo[]): Record<AgentType, string> {
  const map: Partial<Record<AgentType, string>> = {};
  for (const agent of agents) {
    map[agent.agentType] = agent.displayName || agent.agentType;
  }
  return map as Record<AgentType, string>;
}

/**
 * Get display name for a single agent type from discovery data.
 */
export function getAgentLabel(agents: AgentInfo[], agentType: AgentType): string {
  const found = agents.find(a => a.agentType === agentType);
  return found?.displayName || agentType;
}
