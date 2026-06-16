import { useMemo, useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { Session, ContextSnapshot, RollbackResult } from '../../types';
import { IconEdit } from '../Icons';

interface FileChangesProps {
  session: Session | null;
}

interface FileChange {
  path: string;
  added: number;
  removed: number;
}

function extractFileChanges(snapshot: ContextSnapshot | null | undefined): FileChange[] {
  if (!snapshot || !snapshot.filesChanged) return [];
  return snapshot.filesChanged.map(path => ({
    path,
    added: 0,
    removed: 0,
  }));
}

/**
 * Renders the files a session changed, with a one-click "roll back changes"
 * button when a shadow-git checkpoint exists for the session (v1.2 T6). The
 * button restores agent-modified tracked files to HEAD and deletes agent-
 * created untracked files — Claude Code's no-checkpoint blind spot.
 */
export function FileChanges({ session }: FileChangesProps) {
  const files = useMemo(() => extractFileChanges(session?.contextSnapshot), [session?.contextSnapshot]);
  const [hasCheckpoint, setHasCheckpoint] = useState(false);
  const [rolling, setRolling] = useState(false);
  const [result, setResult] = useState<RollbackResult | null>(null);
  const [error, setError] = useState<string | null>(null);

  // Probe whether this session has a checkpoint (drives button visibility).
  // Re-runs on session change; swallows errors (no checkpoint = no button).
  useEffect(() => {
    setHasCheckpoint(false);
    setResult(null);
    setError(null);
    if (!session) return;
    let cancelled = false;
    invoke<unknown | null>('get_checkpoint', {
      projectPath: session.projectPath,
      sessionId: session.id,
    })
      .then((cp) => { if (!cancelled) setHasCheckpoint(!!cp); })
      .catch(() => { if (!cancelled) setHasCheckpoint(false); });
    return () => { cancelled = true; };
  }, [session?.id, session?.projectPath]);

  if (files.length === 0 && !result) return null;

  const handleRollback = async () => {
    if (!session) return;
    const confirmMsg =
      `将回滚 agent 对本次会话的所有改动：\n` +
      `• 恢复 ${files.length} 个被修改的已跟踪文件到改动前\n` +
      `• 删除 agent 新建的未跟踪文件\n\n` +
      `（回滚前会自动保存当前状态，可再次回滚恢复）\n\n确定？`;
    if (!window.confirm(confirmMsg)) return;
    setRolling(true);
    setError(null);
    try {
      const res = await invoke<RollbackResult>('rollback_to_checkpoint', {
        projectPath: session.projectPath,
        sessionId: session.id,
        force: false,
      });
      setResult(res);
    } catch (e) {
      setError(String(e));
    } finally {
      setRolling(false);
    }
  };

  return (
    <div className="agent-block">
      <div className="agent-block-header">
        <span className="agent-block-title">File Changes</span>
        <span className="agent-block-badge">{files.length} files</span>
        {hasCheckpoint && !result && (
          <button
            className="file-change-rollback-btn"
            onClick={handleRollback}
            disabled={rolling}
            title="回滚 agent 对本次会话的文件改动"
          >
            {rolling ? '回滚中…' : '↩ 回滚改动'}
          </button>
        )}
      </div>
      <div className="agent-block-body">
        {result ? (
          <div className="file-change-rollback-result">
            ✓ 已回滚：恢复 {result.restoredFiles.length} 个文件
            {result.removedUntracked.length > 0 && `，删除 ${result.removedUntracked.length} 个新建文件`}
            {result.skipped.length > 0 && `（${result.skipped.length} 个跳过）`}
          </div>
        ) : (
          <div className="file-changes-list">
            {files.map((file, i) => (
              <div key={i} className="file-change-item">
                <span className="file-change-icon"><IconEdit size={14} /></span>
                <span className="file-change-path">{file.path}</span>
                {(file.added > 0 || file.removed > 0) && (
                  <span className="file-change-stats">
                    {file.added > 0 && <span className="file-change-added">+{file.added}</span>}
                    {file.removed > 0 && <span className="file-change-removed">-{file.removed}</span>}
                  </span>
                )}
              </div>
            ))}
          </div>
        )}
        {error && <div className="file-change-rollback-error">回滚失败：{error}</div>}
      </div>
    </div>
  );
}
