import { ModelSelector, type ModelOption } from '../ModelSelector';
import { IconTrash } from '../Icons';

interface ChatHeaderProps {
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

/**
 * ChatHeader — 对话顶栏。砍 CLI（唯一 ReactKernel）+ 移除模式选择器后，只剩
 * ModelSelector（模型选择，保留）+ requestId/LiveDot + 清空。agent 选择器与执行模式
 * 选择器均已移除：agent 固定 Kernel Agent，执行模式不暴露给用户手切（破坏性操作由
 * ApprovalModal 在触发时自动承接）。
 */
export function ChatHeader({
  selectedModel,
  onModelChange,
  modelOptions,
  onClear,
  requestId,
  running = false,
}: ChatHeaderProps) {
  return (
    <div className="chat-header">
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
