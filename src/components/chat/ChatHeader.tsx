import { useAgentStore } from '../../stores/agentStore';
import { ModeSelector, type AgentMode } from '../ModeSelector';
import { ModelSelector, type ModelOption } from '../ModelSelector';
import { IconTrash } from '../Icons';
import type { AgentType } from '../../types';

interface ChatHeaderProps {
  selectedAgent: AgentType | null;
  onAgentChange: (agent: AgentType | null) => void;
  agentMode: AgentMode;
  onModeChange: (mode: AgentMode) => void;
  selectedModel: string;
  onModelChange: (model: string) => void;
  /** Model options sourced from providers.toml (built in ChatView). When
   *  omitted the ModelSelector falls back to its built-in default list. */
  modelOptions?: ModelOption[];
  onClear: () => void;
  /** 当前会话/请求 ID（可选，pi.dev 风格 mono 小标签显示）。
   *  对应 Cursor 3.0 的 requestId 成本透明范式——便于排查日志。 */
  requestId?: string;
  /** 运行中状态（true 时显示 LiveDot）。 */
  running?: boolean;
}

export function ChatHeader({
  selectedAgent,
  onAgentChange,
  agentMode,
  onModeChange,
  selectedModel,
  onModelChange,
  modelOptions,
  onClear,
  requestId,
  running = false,
}: ChatHeaderProps) {
  const agents = useAgentStore((s) => s.agents);
  const installedAgents = agents.filter((a) => a.installed);

  return (
    <div className="chat-header">
      <select
        className="chat-agent-select"
        value={selectedAgent ?? ''}
        onChange={(e) => onAgentChange(e.target.value ? (e.target.value as AgentType) : null)}
      >
        {installedAgents.length === 0 && <option value="">无可用 Agent</option>}
        {installedAgents.map((agent) => (
          <option key={agent.agentType} value={agent.agentType}>
            {agent.displayName}
          </option>
        ))}
        {/* Self-hosted ReactAgent — always available (no CLI to discover), runs
            in-process via the kernel react_chat driver. */}
        <option value="react_kernel">Kernel Agent</option>
      </select>

      <ModeSelector value={agentMode} onChange={onModeChange} />
      <ModelSelector value={selectedModel} onChange={onModelChange} models={modelOptions} />

      {/* requestId 显示（Cursor 3.0 范式）+ 运行中 LiveDot */}
      {(requestId || running) && (
        <span className="chat-header-requestid">
          {running && <span className="chat-header-livedot" aria-hidden="true" />}
          {requestId && <span className="chat-header-requestid-text">{requestId}</span>}
        </span>
      )}

      <button className="chat-clear-btn" title="清空对话" onClick={onClear}>
        <IconTrash size={16} />
      </button>
    </div>
  );
}
