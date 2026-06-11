import { useNavigationStore, type TabId } from '../stores/navigationStore';

const TABS: { id: TabId; label: string; icon: string }[] = [
  { id: 'overview', label: '概览', icon: '◉' },
  { id: 'sessions', label: '对话', icon: '💬' },
  { id: 'timeline', label: '时间线', icon: '◷' },
  { id: 'knowledge', label: '知识库', icon: '⬡' },
];

export function TabBar() {
  const activeTab = useNavigationStore((s) => s.activeTab);
  const setActiveTab = useNavigationStore((s) => s.setActiveTab);

  return (
    <div className="tab-bar">
      {TABS.map((tab) => (
        <button
          key={tab.id}
          className={`tab-bar-item ${activeTab === tab.id ? 'active' : ''}`}
          onClick={() => setActiveTab(tab.id)}
        >
          <span className="tab-bar-icon">{tab.icon}</span>
          <span className="tab-bar-label">{tab.label}</span>
        </button>
      ))}
    </div>
  );
}
