import { invoke } from '@tauri-apps/api/core';

interface ToolButtonProps {
  tool: string;       // claude | cursor | code | terminal
  projectPath: string;
  installed: boolean;
  onClick?: () => void;
}

const TOOL_LABELS: Record<string, string> = {
  claude: 'Claude',
  cursor: 'Cursor',
  code: 'VS Code',
  terminal: 'Terminal',
  finder: 'Finder',
};

const TOOL_ICONS: Record<string, string> = {
  claude: '🤖',
  cursor: '📝',
  code: '💻',
  terminal: '⌨️',
  finder: '📁',
};

export function ToolButton({ tool, projectPath, installed, onClick }: ToolButtonProps) {
  const handleClick = async () => {
    if (!installed) return;

    try {
      switch (tool) {
        case 'claude':
          await invoke('open_terminal', { workingDir: projectPath, command: 'claude' });
          break;
        case 'terminal':
          await invoke('open_terminal', { workingDir: projectPath });
          break;
        case 'cursor':
        case 'code':
          await invoke('open_in_editor', { editor: tool, projectPath });
          break;
        case 'finder':
          await invoke('open_in_finder', { path: projectPath });
          break;
      }
      onClick?.();
    } catch (e) {
      console.error(`启动 ${tool} 失败:`, e);
    }
  };

  return (
    <button
      className={`tool-btn ${installed ? '' : 'disabled'}`}
      onClick={handleClick}
      disabled={!installed}
      title={installed ? `用 ${TOOL_LABELS[tool]} 打开` : `${TOOL_LABELS[tool]} 未安装`}
    >
      <span className="tool-btn-icon">{TOOL_ICONS[tool]}</span>
      <span className="tool-btn-label">{TOOL_LABELS[tool]}</span>
    </button>
  );
}
