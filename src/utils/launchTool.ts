import { invoke } from '@tauri-apps/api/core';
import type { AppSettings } from '../types';

/**
 * 启动指定工具打开项目路径。
 * 从 ToolButton 提取的通用逻辑，供 ToolButton、恢复工作区、一键启动等复用。
 */
export async function launchTool(tool: string, projectPath: string): Promise<void> {
  // 读取设置（自定义路径 + CLI flags）
  let customPath = '';
  let flags = '';
  try {
    const settings = await invoke<AppSettings>('load_settings');
    customPath = settings.tool_paths[tool] || '';
    flags = settings.cli_flags[tool] || '';
  } catch {
    // 读取失败不影响启动
  }

  switch (tool) {
    case 'claude':
    case 'pi':
    case 'codex': {
      const executable = customPath || tool;
      const command = flags ? `${executable} ${flags}` : executable;
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
