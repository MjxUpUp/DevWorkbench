import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { ToolStatus } from '../types';

export function useTools() {
  const [tools, setTools] = useState<ToolStatus[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    invoke<ToolStatus[]>('detect_tools')
      .then(setTools)
      .catch(console.error)
      .finally(() => setLoading(false));
  }, []);

  const isInstalled = (name: string) => {
    const tool = tools.find(t => t.name === name);
    return tool?.installed ?? false;
  };

  return { tools, loading, isInstalled };
}
