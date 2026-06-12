import { useNavigationStore } from '../stores/navigationStore';
import { ChatView } from './chat/ChatView';
import { DashboardView } from './dashboard/DashboardView';
import { OrchestrateView } from './orchestrate/OrchestrateView';
import { SettingsView } from './settings/SettingsView';
import { SkillMarketView } from './skills/SkillMarketView';

export function MainStage() {
  const activeView = useNavigationStore((s) => s.activeView);

  return (
    <main className="main-stage">
      {activeView === 'chat' && <ChatView />}
      {activeView === 'orchestrate' && <OrchestrateView />}
      {activeView === 'skill-market' && <SkillMarketView />}
      {activeView === 'dashboard' && <DashboardView />}
      {activeView === 'settings' && <SettingsView />}
    </main>
  );
}
