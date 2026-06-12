import { useNavigationStore } from '../stores/navigationStore';
import { ChatView } from './chat/ChatView';
import { OverviewTab } from './tabs/OverviewTab';
import { SettingsView } from './settings/SettingsView';

export function MainStage() {
  const activeView = useNavigationStore((s) => s.activeView);
  const project = useNavigationStore((s) => s.activeProject);

  return (
    <main className="main-stage">
      {activeView === 'chat' && <ChatView />}
      {activeView === 'orchestrate' && <PlaceholderView title="Orchestrate" icon="⬡" description="DAG workflow editor — coming soon" />}
      {activeView === 'skill-market' && <PlaceholderView title="Skill Market" icon="◆" description="Skill marketplace — coming soon" />}
      {activeView === 'dashboard' && (
        <div className="dashboard-placeholder">
          <OverviewTab project={project} />
        </div>
      )}
      {activeView === 'settings' && <SettingsView />}
    </main>
  );
}

/** Temporary placeholder for views not yet implemented */
function PlaceholderView({ title, icon, description }: { title: string; icon: string; description: string }) {
  return (
    <div className="placeholder-view">
      <div className="placeholder-icon">{icon}</div>
      <h2 className="placeholder-title">{title}</h2>
      <p className="placeholder-desc">{description}</p>
    </div>
  );
}
