import { invoke } from '@tauri-apps/api/core';

/**
 * 启动指定工具打开项目路径。
 * 从 ToolButton 提取的通用逻辑，供 ToolButton、恢复工作区、一键启动等复用。
 */
export async function launchTool(tool: string, projectPath: string): Promise<void> {
  switch (tool) {
    case 'claude':
      return invoke('open_terminal', { workingDir: projectPath, command: 'claude' });
    case 'terminal':
      return invoke('open_terminal', { workingDir: projectPath });
    case 'cursor':
    case 'code':
      return invoke('open_in_editor', { editor: tool, projectPath });
    case 'finder':
      return invoke('open_in_finder', { path: projectPath });
    default:
      throw new Error(`未知工具: ${tool}`);
  }
}
