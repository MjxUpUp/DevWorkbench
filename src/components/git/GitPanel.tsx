import { useState, useEffect, useCallback, useRef, Component, type ReactNode } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useToast } from '../Toast';
import { IconBranch } from '../Icons';
import { isTauri } from '../../utils/env';
import type { GitStatus } from '../../types';

/**
 * Right-side Git tool panel (rendered only inside the task view).
 *
 * Mirrors the target mockup: a "更改" indicator with +/- line counts, the
 * current branch, and a "提交" button that opens a terminal in the project
 * directory so the user can stage and commit. Refreshes on an interval while
 * visible and whenever the active project changes.
 *
 * Wrapped in a local ErrorBoundary so a backend/git hiccup degrades to a small
 * panel message instead of blanking the whole app (whitepage).
 */
export function GitPanel({ projectPath }: { projectPath: string | null }) {
  return (
    <ErrorBoundary fallback={<aside className="git-panel git-panel-empty"><div className="git-panel-header">Git 工具</div><div className="git-panel-placeholder">Git 状态读取失败</div></aside>}>
      <GitPanelInner projectPath={projectPath} />
    </ErrorBoundary>
  );
}

class ErrorBoundary extends Component<{ children: ReactNode; fallback: ReactNode }, { hasError: boolean }> {
  state = { hasError: false };
  static getDerivedStateFromError() { return { hasError: true }; }
  componentDidCatch(error: Error) { console.error('[GitPanel] render error:', error); }
  componentDidUpdate(prev: { children: ReactNode }) {
    // Reset when the project changes so a transient error doesn't stick.
    if (prev.children !== this.props.children && this.state.hasError) {
      this.setState({ hasError: false });
    }
  }
  render() { return this.state.hasError ? this.props.fallback : this.props.children; }
}

function GitPanelInner({ projectPath }: { projectPath: string | null }) {
  const [status, setStatus] = useState<GitStatus | null>(null);
  const [loading, setLoading] = useState(false);
  const { info, error } = useToast();
  // Guards so a stale async result can't overwrite a newer one, and so the
  // interval is skipped while a fetch is already in flight (avoids pile-up if
  // the backend is slow on a large repo).
  const reqIdRef = useRef(0);
  const inFlightRef = useRef(false);

  const refresh = useCallback(async () => {
    if (!projectPath || !isTauri()) { setStatus(null); return; }
    if (inFlightRef.current) return; // don't pile up concurrent fetches
    inFlightRef.current = true;
    const myId = ++reqIdRef.current;
    setLoading(true);
    try {
      const s = await invoke<GitStatus>('get_git_status', { projectPath });
      // Only apply if this is still the latest request.
      if (myId === reqIdRef.current) setStatus(s);
    } catch {
      if (myId === reqIdRef.current) setStatus(null);
    } finally {
      if (myId === reqIdRef.current) setLoading(false);
      inFlightRef.current = false;
    }
  }, [projectPath]);

  // Load on project change, then poll every 15s for fresh diff counts.
  useEffect(() => {
    reqIdRef.current++; // invalidate any in-flight request from the previous project
    refresh();
    if (!projectPath) return;
    const id = setInterval(refresh, 15000);
    return () => clearInterval(id);
  }, [projectPath, refresh]);

  const handleCommit = async () => {
    if (!projectPath) return;
    try {
      await invoke('open_terminal', { workingDir: projectPath });
      info('已在项目目录打开终端，可执行 git add / commit');
    } catch (e) {
      error(`打开终端失败：${e}`);
    }
  };

  // No project / not a repo
  if (!projectPath) {
    return (
      <aside className="git-panel git-panel-empty">
        <div className="git-panel-header">Git 工具</div>
        <div className="git-panel-placeholder">选择项目后查看 Git 状态</div>
      </aside>
    );
  }

  if (!status && loading) {
    return (
      <aside className="git-panel git-panel-empty">
        <div className="git-panel-header">Git 工具</div>
        <div className="git-panel-placeholder">读取中…</div>
      </aside>
    );
  }

  if (!status) {
    return (
      <aside className="git-panel git-panel-empty">
        <div className="git-panel-header">Git 工具</div>
        <div className="git-panel-placeholder">非 Git 仓库或无法读取</div>
      </aside>
    );
  }

  // Defensive: tolerate a backend payload missing the new numeric fields.
  const insertions = Number(status.insertions ?? 0);
  const deletions = Number(status.deletions ?? 0);
  const dirty = status.isDirty || insertions > 0 || deletions > 0;

  return (
    <aside className="git-panel">
      <div className="git-panel-header">
        <span className="git-panel-title">Git 工具</span>
      </div>

      <div className="git-panel-body">
        {/* Changes summary: ● 更改 +xxxx -xxxx */}
        <div className={`git-status-row ${dirty ? 'dirty' : 'clean'}`}>
          <span className="git-status-dot" aria-hidden />
          <span className="git-status-label">{dirty ? '更改' : '干净'}</span>
          <span className="git-status-counts">
            <span className="git-count ins">+{insertions}</span>
            <span className="git-count del">-{deletions}</span>
          </span>
        </div>

        {/* Current branch */}
        <div className="git-branch-row" title="当前分支">
          <IconBranch size={13} className="git-branch-icon" />
          <span className="git-branch-name">{status.branch}</span>
        </div>

        {/* Ahead / behind (only when relevant) */}
        {(status.ahead > 0 || status.behind > 0) && (
          <div className="git-sync-row">
            {status.ahead > 0 && <span className="git-sync-ahead">↑ {status.ahead}</span>}
            {status.behind > 0 && <span className="git-sync-behind">↓ {status.behind}</span>}
            <span className="git-sync-hint">相对上游</span>
          </div>
        )}

        <button
          className="git-commit-btn"
          onClick={handleCommit}
          disabled={!dirty}
          title={dirty ? '在项目目录打开终端进行提交' : '没有可提交的更改'}
        >
          提交 ...
        </button>
      </div>
    </aside>
  );
}
