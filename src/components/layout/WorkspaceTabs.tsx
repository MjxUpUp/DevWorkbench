import { useProjectStore } from '../../stores/projectStore';
import { useNavigationStore } from '../../stores/navigationStore';

/**
 * WorkspaceTabs — 底部 Excel-sheet 式工作区切换器（方案 B 核心）。
 *
 * 抛弃侧栏的项目列表，工作区（磁盘项目）改为底部 sheet tab 切换，对齐 Excel/IDE 底部
 * 工作表切换的肌肉记忆。每张 tab = 一个工作区；active tab 顶部 accent stripe 标识当前
 * 作用域；末尾「+」复用 ActivityBar 的「添加工作区」入口（setAddProjectOpen）。
 *
 * 数据真相源：projectStore.projects + navigationStore.activeProject。点击 tab →
 * selectProject(p)（store 内已清空 selectedConversationId，工作区切换重置会话作用域）。
 */
export function WorkspaceTabs() {
  const projects = useProjectStore((s) => s.projects);
  const activeProject = useNavigationStore((s) => s.activeProject);
  const selectProject = useNavigationStore((s) => s.selectProject);
  const setAddProjectOpen = useNavigationStore((s) => s.setAddProjectOpen);

  return (
    <div className="workspace-tabs" role="tablist" aria-label="工作区切换" data-testid="workspace-tabs">
      {projects.map((p) => {
        const active = activeProject?.path === p.path;
        return (
          <button
            key={p.path}
            type="button"
            role="tab"
            aria-selected={active}
            className={`ws-tab${active ? ' ws-tab-active' : ''}`}
            onClick={() => selectProject(p)}
            title={p.path}
            data-testid="ws-tab"
          >
            <span className="ws-tab-name">{p.name}</span>
          </button>
        );
      })}
      <button
        type="button"
        className="ws-tab ws-tab-add"
        onClick={() => setAddProjectOpen(true)}
        title="添加工作区"
        aria-label="添加工作区"
        data-testid="ws-tab-add"
      >
        +
      </button>
    </div>
  );
}
