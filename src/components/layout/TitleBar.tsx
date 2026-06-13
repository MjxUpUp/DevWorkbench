import { useState, useEffect } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useNavigationStore, type ViewId } from '../../stores/navigationStore';
import { IconBranch, IconChevronRight } from '../Icons';
import type { GitStatus } from '../../types';

/**
 * Unified top title bar — spans the full window width above the main content.
 * Mirrors zcode's window-level header: brand mark + project breadcrumb on the
 * left, git branch context on the right. Rendered once in App so every view
 * (chat / orchestrate / skills / dashboard / settings) shares the same chrome
 * instead of each shipping its own ad-hoc header.
 */
const VIEW_LABELS: Record<ViewId, string> = {
  chat: '对话',
  orchestrate: '编排',
  'skill-market': '技能市场',
  dashboard: '仪表盘',
  settings: '设置',
};

export function TitleBar() {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const activeView = useNavigationStore((s) => s.activeView);
  const [gitBranch, setGitBranch] = useState<string>('');

  useEffect(() => {
    if (!activeProject?.path) { setGitBranch(''); return; }
    invoke<GitStatus>('get_git_status', { projectPath: activeProject.path })
      .then((status) => setGitBranch(status.branch))
      .catch(() => setGitBranch(''));
  }, [activeProject?.path]);

  const projectName = activeProject?.name ?? 'Dev Workbench';

  return (
    <header className="title-bar" role="banner">
      <div className="title-bar-left">
        <span className="title-bar-brand" title="Dev Workbench">DW</span>
        <span className="title-bar-crumb">{projectName}</span>
        <IconChevronRight size={14} className="title-bar-sep" />
        <span className="title-bar-view">{VIEW_LABELS[activeView]}</span>
      </div>
      <div className="title-bar-right">
        {gitBranch && (
          <span className="title-bar-branch" title="当前 Git 分支">
            <IconBranch size={13} />
            {gitBranch}
          </span>
        )}
      </div>
    </header>
  );
}
