import { invoke } from '@tauri-apps/api/core';
import { IconTerminal, IconCode, IconFolderOpen, IconSparkles } from './Icons';
import { useToast } from './Toast';

interface ToolButtonProps {
  tool: string;
  projectPath: string;
  installed: boolean;
  onClick?: () => void;
}

const TOOL_LABELS: Record<string, string> = {
  claude: 'Claude',
  cursor: 'Cursor',
  code: 'VSCode',
  terminal: 'Term',
  finder: 'Files',
};

const TOOL_ICONS: Record<string, typeof IconTerminal> = {
  claude: IconSparkles,
  cursor: IconCode,
  code: IconCode,
  terminal: IconTerminal,
  finder: IconFolderOpen,
};

export function ToolButton({ tool, projectPath, installed, onClick }: ToolButtonProps) {
  const IconComponent = TOOL_ICONS[tool] || IconTerminal;
  const toast = useToast();

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
      toast.error(`启动 ${TOOL_LABELS[tool]} 失败: ${e}`);
    }
  };

  return (
    <button
      className={`tool-btn ${installed ? '' : 'disabled'}`}
      onClick={handleClick}
      disabled={!installed}
      title={installed ? `用 ${TOOL_LABELS[tool]} 打开` : `${TOOL_LABELS[tool]} 未安装`}
    >
      <span className="tool-btn-icon"><IconComponent size={14} /></span>
      <span className="tool-btn-label">{TOOL_LABELS[tool]}</span>
    </button>
  );
}
