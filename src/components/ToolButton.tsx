import { IconTerminal, IconCode, IconFolderOpen, IconSparkles } from './Icons';
import { useToast } from './Toast';
import { launchTool } from '../utils/launchTool';

interface ToolButtonProps {
  tool: string;
  projectPath: string;
  installed: boolean;
  onClick?: (toolName: string) => void;
}

const TOOL_LABELS: Record<string, string> = {
  claude: 'Claude',
  cursor: 'Cursor',
  code: 'VSCode',
  finder: 'Files',
};

const TOOL_ICONS: Record<string, typeof IconTerminal> = {
  claude: IconSparkles,
  cursor: IconCode,
  code: IconCode,
  finder: IconFolderOpen,
};

export function ToolButton({ tool, projectPath, installed, onClick }: ToolButtonProps) {
  const IconComponent = TOOL_ICONS[tool] || IconTerminal;
  const toast = useToast();

  const handleClick = async () => {
    if (!installed) return;

    try {
      await launchTool(tool, projectPath);
      onClick?.(tool);
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

export { TOOL_LABELS, TOOL_ICONS };
