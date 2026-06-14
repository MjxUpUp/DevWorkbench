import { useNavigationStore } from '../stores/navigationStore';
import { ChatView } from './chat/ChatView';
import { SearchView } from './search/SearchView';
import { SkillMarketView } from './skills/SkillMarketView';
import { OrchestrateView } from './orchestrate/OrchestrateView';
import { GitPanel } from './git/GitPanel';

/**
 * Main stage view router.
 *
 * Views: task (chat) / search / skills / orchestrate. (Settings renders as a
 * full-screen overlay above the grid — see App.tsx.)
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
        {activeView === 'search' && <SearchView />}
        {activeView === 'skills' && <SkillMarketView />}
        {activeView === 'orchestrate' && <OrchestrateView />}
      </div>
      {isTask && <GitPanel projectPath={activeProject?.path ?? null} />}
    </main>
  );
}
