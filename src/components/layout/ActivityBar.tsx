import { useNavigationStore, type ViewId } from '../../stores/navigationStore';

const VIEWS: { id: ViewId; label: string; icon: string }[] = [
  { id: 'chat', label: 'Chat', icon: '◉' },
  { id: 'orchestrate', label: 'Orch', icon: '⬡' },
  { id: 'skill-market', label: 'Mkt', icon: '◆' },
  { id: 'dashboard', label: 'Dash', icon: '▦' },
  { id: 'settings', label: 'Set', icon: '⚙' },
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
            <span className="activity-bar-icon">{view.icon}</span>
          </button>
        ))}
      </div>
      <div className="activity-bar-bottom">
        <button className="activity-bar-item" title="User" aria-label="User settings">
          <span className="activity-bar-icon">◐</span>
        </button>
      </div>
    </nav>
  );
}
