import { useNavigationStore } from '../stores/navigationStore';
import { ChatView } from './chat/ChatView';
import { OrchestrateView } from './orchestrate/OrchestrateView';
import { TraceView } from './trace/TraceView';
import { GitPanel } from './git/GitPanel';

/**
 * Main stage view router.
 *
 * Views: task (chat) / orchestrate / trace. (技能目录已下放到设置页统一管理;
 * Settings renders as a full-screen overlay above the grid — see App.tsx.
 * Search is a CommandPalette modal, not a routed view — see Sidebar's 搜索 item.)
 *
 * The task view is the only one that shows the right-side Git tool panel — the
 * stage becomes a 2-column grid (chat | git) in that mode. Other views are
 * single-pane.
 */
export function MainStage() {
  const activeView = useNavigationStore((s) => s.activeView);
  const activeProject = useNavigationStore((s) => s.activeProject);

  const isTask = activeView === 'task';

  return (
    <main className={`main-stage${isTask ? ' has-git-panel' : ''}`}>
      <div className="main-stage-body">
        {activeView === 'task' && <ChatView />}
        {activeView === 'orchestrate' && <OrchestrateView />}
        {activeView === 'trace' && <TraceView />}
      </div>
      {isTask && <GitPanel projectPath={activeProject?.path ?? null} />}
    </main>
  );
}
