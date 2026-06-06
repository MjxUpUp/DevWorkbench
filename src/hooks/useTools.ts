import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ToolStatus } from '../types';

export function useTools() {
  const [tools, setTools] = useState<ToolStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
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
