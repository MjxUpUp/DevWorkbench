import { useState, useEffect } from 'react';
import { getVersion } from '@tauri-apps/api/app';
import { invoke } from '@tauri-apps/api/core';
import { useNavigationStore } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';
import type { GitStatus } from '../../types';

export function StatusBar() {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const sessions = useAgentStore((s) => s.sessions);
  const [appVersion, setAppVersion] = useState('');
  const [gitBranch, setGitBranch] = useState<string>('');

  // Scope running state to the active project. Without this filter, a session
  // still running in project A keeps the status bar showing "X running" after
  // the user switches to project B — the cross-project leak caught in the
  // acceptance round.
  const projectSessions = activeProject
    ? sessions.filter(s => s.projectPath === activeProject.path)
    : sessions;
  const runningSession = projectSessions.find(s => s.status === 'running');
  const projectName = activeProject?.name ?? 'No project';
  const runningCount = projectSessions.filter(s => s.status === 'running').length;

  useEffect(() => { getVersion().then(v => setAppVersion(v)).catch(() => setAppVersion('dev')); }, []);

  // Read git branch via Tauri command
  useEffect(() => {
    if (!activeProject?.path) { setGitBranch(''); return; }
    invoke<GitStatus>('get_git_status', { projectPath: activeProject.path })
      .then(status => setGitBranch(status.branch))
      .catch(() => setGitBranch(''));
  }, [activeProject?.path]);

  // Resolve display model name from running session or last session
  const modelDisplay = runningSession?.model || projectSessions[projectSessions.length - 1]?.model || '';

  return (
    <footer className="status-bar">
      <div className="status-bar-left">
        <span className="status-bar-item">
          <span className="status-dot" />
          {projectName}
        </span>
        {runningCount > 0 && (
          <span className="status-bar-item running">
            <span className="status-dot" />
            {runningCount} running
          </span>
        )}
      </div>
      <div className="status-bar-right">
        {modelDisplay && (
          <>
            <span className="status-bar-item model-name">{modelDisplay}</span>
            <span className="status-bar-divider" />
          </>
        )}
        <span className="status-bar-item">
          {runningSession ? '运行中' : '就绪'}
        </span>
        {gitBranch && (
          <>
            <span className="status-bar-divider" />
            <span className="status-bar-item branch-label">{gitBranch}</span>
          </>
        )}
        {appVersion && (
          <>
            <span className="status-bar-divider" />
            <span className="status-bar-item">v{appVersion}</span>
          </>
        )}
      </div>
    </footer>
  );
}
