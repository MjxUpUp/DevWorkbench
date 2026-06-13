import { useNavigationStore, type ViewId } from '../../stores/navigationStore';
import { IconChat, IconOrchestrate, IconSkillMarket, IconDashboard, IconSettings, IconUser } from '../Icons';
import type { IconProps } from '../Icons';

const VIEWS: { id: ViewId; label: string; Icon: React.FC<IconProps> }[] = [
  { id: 'chat', label: 'Chat', Icon: IconChat },
  { id: 'orchestrate', label: 'Orch', Icon: IconOrchestrate },
  { id: 'skill-market', label: 'Mkt', Icon: IconSkillMarket },
  { id: 'dashboard', label: 'Dash', Icon: IconDashboard },
  { id: 'settings', label: 'Set', Icon: IconSettings },
];

export function ActivityBar() {
  const activeView = useNavigationStore((s) => s.activeView);
  const setActiveView = useNavigationStore((s) => s.setActiveView);

  return (
    <nav className="activity-bar" role="navigation" aria-label="Main navigation">
      <div className="activity-bar-top">
        <div className="activity-bar-logo" title="Dev Workbench">DW</div>
        {VIEWS.map((view) => (
          <button
            key={view.id}
            className={`activity-bar-item ${activeView === view.id ? 'active' : ''}`}
            onClick={() => setActiveView(view.id)}
            title={view.label}
            aria-label={view.label}
            aria-selected={activeView === view.id}
          >
            <view.Icon size={18} />
          </button>
        ))}
      </div>
      <div className="activity-bar-bottom">
        <button className="activity-bar-item" title="User" aria-label="User settings">
          <IconUser size={18} />
        </button>
      </div>
    </nav>
  );
}
