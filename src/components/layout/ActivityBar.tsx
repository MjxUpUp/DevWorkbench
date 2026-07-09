import { useNavigationStore, type ViewId } from '../../stores/navigationStore';
import { IconPlus, IconChat, IconSearch, IconSettings } from '../Icons';

/**
 * ActivityBar — 48px 纯图标导航竖条（对齐原型 axis-workbench.html）。
 *
 * 视图导航从宽 Sidebar 顶部 nav 抽出，独立成 VS Code 式 activity-bar 常驻最左列。
 * 砍 DAG 编排画布后，Orchestrate 图标移除——不再有独立编排入口。
 *
 * 图标自上而下：
 *  - 新建会话（+，强调态）→ task 视图 + 清空当前对话（与 Ctrl+N 同语义）
 *  - Task / Trace：视图切换（PlanBar mode-segmented 同源）
 *  - 搜索 → 打开命令面板（查对话历史）
 *  - 设置 → 设置页
 */
export function ActivityBar() {
  const activeView = useNavigationStore((s) => s.activeView);
  const setActiveView = useNavigationStore((s) => s.setActiveView);
  const selectConversation = useNavigationStore((s) => s.selectConversation);
  const setCommandPaletteOpen = useNavigationStore((s) => s.setCommandPaletteOpen);
  const commandPaletteOpen = useNavigationStore((s) => s.commandPaletteOpen);
  const setAddProjectOpen = useNavigationStore((s) => s.setAddProjectOpen);

  // 新建会话：清空当前对话选择 + 进 task 视图（与 Ctrl+N 同语义）。
  const handleNewChat = () => {
    selectConversation(null);
    setActiveView('task');
  };

  const isActive = (view: ViewId) => (activeView === view ? 'active' : '');

  return (
    <nav className="activity-bar" aria-label="视图导航" data-testid="activity-bar">
      <button
        className="ab-icon ab-new"
        onClick={handleNewChat}
        title="新建会话"
        aria-label="新建会话"
        type="button"
        data-testid="ab-new"
      >
        <IconPlus size={18} />
      </button>
      <button
        className="ab-icon"
        onClick={() => setAddProjectOpen(true)}
        title="添加工作区"
        aria-label="添加工作区"
        type="button"
        data-testid="ab-add-workspace"
      >
        {/* folder + plus —— 添加磁盘项目为工作区（原 Sidebar UserMenu 的「添加项目」入口）*/}
        <svg width={18} height={18} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round">
          <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
          <line x1="12" y1="11" x2="12" y2="17" />
          <line x1="9" y1="14" x2="15" y2="14" />
        </svg>
      </button>

      <div className="ab-spacer" />

      <button
        className={`ab-icon ${isActive('task')}`}
        onClick={() => setActiveView('task')}
        title="Task · 轴线化工作面"
        aria-label="Task 视图"
        aria-pressed={activeView === 'task'}
        type="button"
        data-testid="ab-task"
      >
        <IconChat size={18} />
      </button>
      <button
        className={`ab-icon ${isActive('trace')}`}
        onClick={() => setActiveView('trace')}
        title="Trace · 可观测"
        aria-label="Trace 视图"
        aria-pressed={activeView === 'trace'}
        type="button"
        data-testid="ab-trace"
      >
        {/* 波形/心电图图标（原型 axis-workbench.html:372）—— Icons.tsx 无对应项，内联 */}
        <svg width={18} height={18} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth={1.8} strokeLinecap="round" strokeLinejoin="round">
          <path d="M3 12h4l3-9 4 18 3-9h4" />
        </svg>
      </button>
      <button
        className={`ab-icon ${commandPaletteOpen ? 'active' : ''}`}
        onClick={() => setCommandPaletteOpen(true)}
        title="搜索 · 命令面板"
        aria-label="搜索对话历史"
        aria-expanded={commandPaletteOpen}
        type="button"
        data-testid="ab-search"
      >
        <IconSearch size={18} />
      </button>

      <div className="ab-spacer" />

      <button
        className={`ab-icon ${isActive('settings')}`}
        onClick={() => setActiveView('settings')}
        title="设置"
        aria-label="设置"
        aria-pressed={activeView === 'settings'}
        type="button"
        data-testid="ab-settings"
      >
        <IconSettings size={18} />
      </button>
    </nav>
  );
}
