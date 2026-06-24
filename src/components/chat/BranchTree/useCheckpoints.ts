import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { BranchNode } from '../../../types';

/**
 * useCheckpoints — 批量 probe 一组 session 节点是否有 git checkpoint。
 *
 * 对每个 BranchNode 调 get_checkpoint，返回有 checkpoint 的 sessionId 集合。
 * BranchTree 用这个集合显示 ◆ 标记。
 *
 * 注意：这是 N 次 invoke（每个节点一次 probe）。对大对话树可能有性能影响，
 * 但 get_checkpoint 是轻量查询（读 git SHA），实测可接受。如果未来需要优化，
 * 可加一个后端 batch_get_checkpoints 命令。
 */
export function useCheckpoints(branches: BranchNode[], projectPath: string | null) {
  const [checkpointIds, setCheckpointIds] = useState<Set<string>>(new Set());

  useEffect(() => {
    if (!projectPath || branches.length === 0) {
      setCheckpointIds(new Set());
      return;
    }
    let cancelled = false;
    const ids = new Set<string>();
    // 并发 probe 所有节点（get_checkpoint 是只读查询，并发安全）
    Promise.all(
      branches.map((b) =>
        invoke<unknown | null>('get_checkpoint', { projectPath, sessionId: b.id })
          .then((cp) => { if (cp) ids.add(b.id); })
          .catch(() => { /* 单个 probe 失败不影响整体 */ })
      )
    ).then(() => {
      if (!cancelled) setCheckpointIds(ids);
    });
    return () => { cancelled = true; };
  }, [branches, projectPath]);

  return checkpointIds;
}
