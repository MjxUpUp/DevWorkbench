import { invoke } from '@tauri-apps/api/core';
import type { AppSettings } from '../types';

/**
 * 启动指定工具打开项目路径。
 * 从 ToolButton 提取的通用逻辑，供 ToolButton、恢复工作区、一键启动等复用。
 */
export async function launchTool(tool: string, projectPath: string): Promise<void> {
  // 读取 CLI flags 设置
  let flags = '';
  try {
    const settings = await invoke<AppSettings>('load_settings');
    flags = settings.cli_flags[tool] || '';
  } catch {
    // 读取失败不影响启动
  }

  switch (tool) {
    case 'claude':
    case 'pi':
    case 'codex': {
      const command = flags ? `${tool} ${flags}` : tool;
      return invoke('open_terminal', { workingDir: projectPath, command });
    }
    case 'cursor':
    case 'code':
      return invoke('open_in_editor', { editor: tool, projectPath });
    case 'finder':
      return invoke('open_in_finder', { path: projectPath });
    default:
      throw new Error(`未知工具: ${tool}`);
  }
}
