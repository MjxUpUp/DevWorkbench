import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { useNavigationStore } from '../../stores/navigationStore';
import { IconBranch } from '../Icons';
import { isTauri } from '../../utils/env';
import type { GitStatus } from '../../types';
import { WindowControls } from './WindowControls';

/**
 * Unified top title bar — spans the full window width above the main content.
 * Frameless: the bar itself is the drag region (`data-tauri-drag-region`) and
 * carries the window controls on the right.
 *
 * Layout (aligned to the target mockup):
 *   [brand mark, toggles the left column] · [breadcrumb: 项目 / 分支]   [window controls]
 *
 * Git branch lives in the breadcrumb here.
 */
export function TitleBar() {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const setActiveView = useNavigationStore((s) => s.setActiveView);
  const selectConversation = useNavigationStore((s) => s.selectConversation);
  const [gitBranch, setGitBranch] = useState<string>('');
  const [maximized, setMaximized] = useState(false);

  useEffect(() => {
    if (!activeProject?.path) { setGitBranch(''); return; }
    invoke<GitStatus>('get_git_status', { projectPath: activeProject.path })
      .then((status) => setGitBranch(status.branch))
      .catch(() => setGitBranch(''));
  }, [activeProject?.path]);

  // Track window maximized state — toggles the restore/maximize icon and applies
  // `.window-maximized` to the app root. Frameless windows fill the screen
  // edge-to-edge when maximized, so CSS restores inner spacing on that class.
  useEffect(() => {
    if (!isTauri()) return;
    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;
    let cancelled = false;
    const sync = () => win.isMaximized().then((m) => { if (!cancelled) setMaximized(m); }).catch(() => {});
    sync();
    win.onResized(sync).then((u) => { if (cancelled) u(); else unlisten = u; });
    return () => { cancelled = true; unlisten?.(); };
  }, []);

  useEffect(() => {
    document.querySelector('.app')?.classList.toggle('window-maximized', maximized);
  }, [maximized]);

  return (
    <header className="title-bar" data-tauri-drag-region role="banner">
      <div className="title-bar-left" data-tauri-drag-region>
        <button
          className="title-bar-brand"
          onClick={() => { setActiveView('task'); selectConversation(null); }}
          title="返回任务视图"
          aria-label="返回任务视图"
          type="button"
        >
          DW
        </button>
        {/* Breadcrumb: 项目名 / 分支 — mirrors the target mockup's top-bar context */}
        {activeProject && (
          <span className="title-bar-breadcrumb" data-tauri-drag-region>
            <span className="title-bar-crumb">{activeProject.name}</span>
            {gitBranch && (
              <>
                <span className="title-bar-crumb-sep">/</span>
                <span className="title-bar-crumb title-bar-crumb-branch" title="当前 Git 分支">
                  <IconBranch size={12} className="title-bar-crumb-icon" />
                  {gitBranch}
                </span>
              </>
            )}
          </span>
        )}
      </div>
      {/* right side is NOT a drag region — protects the window controls from hijack */}
      <div className="title-bar-right">
        <WindowControls maximized={maximized} />
      </div>
    </header>
  );
}
