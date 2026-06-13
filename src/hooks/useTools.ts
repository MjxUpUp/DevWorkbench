import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ToolStatus } from '../types';
import { isTauri } from '../utils/env';

export function useTools() {
  const [tools, setTools] = useState<ToolStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    // No Tauri IPC in a plain browser preview — skip detection and leave an
    // empty tool list rather than surfacing a "工具检测失败" error.
    if (!isTauri()) {
      setTools([]);
      setError(null);
      setLoading(false);
      return;
    }
    invoke<ToolStatus[]>('detect_tools')
      .then(data => { setTools(data); setError(null); })
      .catch(e => setError(`工具检测失败: ${e}`))
      .finally(() => setLoading(false));
  }, []);

  const isInstalled = (name: string) => {
    const tool = tools.find(t => t.name === name);
    return tool?.installed ?? false;
  };

  return { tools, loading, error, isInstalled };
}
