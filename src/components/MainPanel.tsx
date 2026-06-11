import { useNavigationStore } from '../stores/navigationStore';
import { TabBar } from './TabBar';
import { OverviewTab } from './tabs/OverviewTab';
import { SessionsTab } from './tabs/SessionsTab';
import { TimelineTab } from './tabs/TimelineTab';
import { KnowledgeTab } from './tabs/KnowledgeTab';

export function MainPanel() {
  const activeTab = useNavigationStore((s) => s.activeTab);
  const project = useNavigationStore((s) => s.activeProject);

  return (
    <div className="main-panel">
      <TabBar />
      <div className="main-panel-content">
        {activeTab === 'overview' ? <OverviewTab project={project} /> :
         activeTab === 'sessions' ? <SessionsTab /> :
         activeTab === 'timeline' ? <TimelineTab project={project} /> :
         <KnowledgeTab project={project} />}
      </div>
    </div>
  );
}
