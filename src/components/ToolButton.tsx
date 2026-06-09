import { IconTerminal, IconCode, IconFolderOpen, IconSparkles, IconBrain, IconCpu } from './Icons';
import { useToast } from './Toast';
import { launchTool } from '../utils/launchTool';

interface ToolButtonProps {
  tool: string;
  projectPath: string;
  installed: boolean;
  label?: string;
  onClick?: (toolName: string) => void;
}

const DEFAULT_LABELS: Record<string, string> = {
  claude: 'Claude',
  cursor: 'Cursor',
  code: 'VSCode',
  finder: 'Files',
  pi: 'Pi',
  codex: 'Codex',
  gemini: 'Gemini',
  'github-copilot-cli': 'Copilot',
  'cursor-agent': 'Cursor',
  'qwen-code': 'Qwen',
};

const TOOL_ICONS: Record<string, typeof IconTerminal> = {
  claude: IconSparkles,
  cursor: IconCode,
  'cursor-agent': IconCode,
  code: IconCode,
  finder: IconFolderOpen,
  pi: IconBrain,
  codex: IconCpu,
  gemini: IconSparkles,
  'github-copilot-cli': IconBrain,
  'qwen-code': IconCpu,
};

export function ToolButton({ tool, projectPath, installed, label, onClick }: ToolButtonProps) {
  const IconComponent = TOOL_ICONS[tool] || IconTerminal;
  const displayLabel = label || DEFAULT_LABELS[tool] || tool;
  const toast = useToast();

  const handleClick = async () => {
    if (!installed) return;

    try {
      await launchTool(tool, projectPath);
      onClick?.(tool);
    } catch (e) {
      toast.error(`启动 ${displayLabel} 失败: ${e}`);
    }
  };

  return (
    <button
      className={`tool-btn ${installed ? '' : 'disabled'}`}
      onClick={handleClick}
      disabled={!installed}
      title={installed ? `用 ${displayLabel} 打开` : `${displayLabel} 未安装`}
    >
      <span className="tool-btn-icon"><IconComponent size={14} /></span>
      <span className="tool-btn-label">{displayLabel}</span>
    </button>
  );
}
