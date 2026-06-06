import { useState, useEffect, useCallback, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { GitStatus, Project } from '../types';

// 缓存 git status，避免频繁重新请求
interface CachedStatus {
  status: GitStatus | null;
  fetchedAt: number;
}

const CACHE_TTL_MS = 60_000; // 60 秒缓存

export function useGitStatus(projects: Project[]) {
  const [gitStatusMap, setGitStatusMap] = useState<Record<string, GitStatus | null>>({});
  const [loading, setLoading] = useState(false);
  const cache = useRef<Record<string, CachedStatus>>({});
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);

  const refresh = useCallback(async (force = false) => {
    if (projects.length === 0) return;

    const now = Date.now();
    const pathsToFetch: string[] = [];

    for (const p of projects) {
      const cached = cache.current[p.path];
      if (force || !cached || now - cached.fetchedAt > CACHE_TTL_MS) {
        pathsToFetch.push(p.path);
      }
    }

    if (pathsToFetch.length === 0) return;

    setLoading(true);
    try {
      const results = await invoke<[string, GitStatus | null][]>('batch_get_git_status', {
        projectPaths: pathsToFetch,
      });

      const updates: Record<string, GitStatus | null> = {};
      for (const [path, status] of results) {
        updates[path] = status;
        cache.current[path] = { status, fetchedAt: now };
      }

      setGitStatusMap(prev => ({ ...prev, ...updates }));
    } catch {
      // 静默失败，保持上次缓存
    } finally {
      setLoading(false);
    }
  }, [projects]);

  // 项目列表变化时刷新
  useEffect(() => {
    refresh();
  }, [refresh]);

  // 每 60 秒自动刷新
  useEffect(() => {
    if (timerRef.current) clearInterval(timerRef.current);
    timerRef.current = setInterval(() => refresh(), CACHE_TTL_MS);
    return () => {
      if (timerRef.current) clearInterval(timerRef.current);
    };
  }, [refresh]);

  const getStatus = useCallback((projectPath: string): GitStatus | null => {
    const cached = cache.current[projectPath];
    if (cached) return cached.status;
    return gitStatusMap[projectPath] ?? null;
  }, [gitStatusMap]);

  return { gitStatusMap, loading, refresh, getStatus };
}
