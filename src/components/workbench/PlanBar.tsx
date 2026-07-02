import { useNavigationStore, type ViewId } from '../../stores/navigationStore';
import styles from './workbench.module.css';

/**
 * PlanBar — 轴线 A「谁持有 plan」（起底重构 块1 骨架）。
 *
 * 调研启示 1/5/7/8：把"谁持有 plan / 编排在何处 / 中间结果落在何处"做成 UI 一等
 * 轴线。骨架阶段：按当前视图派生运行模式（Chat Agent / DAG Script / 观测），并标注
 * 该模式下 plan 与中间结果的物理落点（context window vs 脚本变量 vs 节点输出）。
 * 块2 填实：plan 大纲展开 + per-run 预算/迭代 transparency。
 *
 * 借鉴：Claude Code "who holds the plan" 设计轴（调研 claim18，3-0 验证）+
 * Anthropic workflow-vs-agent 二分（claim11）。反例：不默认所有任务走 chat agent
 * loop（4-15x 成本膨胀，claim2）。
 */
type ModeInfo = {
  label: string;
  /** plan 当前由谁持有 */
  planLoc: string;
  /** 中间结果落在何处 */
  resultsLoc: string;
};

const MODE_BY_VIEW: Record<string, ModeInfo> = {
  task: {
    label: 'Chat Agent',
    planLoc: 'plan ∈ LLM context',
    resultsLoc: 'results ∈ 对话历史',
  },
  orchestrate: {
    label: 'DAG Script',
    planLoc: 'plan ∈ 脚本变量',
    resultsLoc: 'results ∈ 节点输出',
  },
  trace: {
    label: '观测',
    planLoc: '—',
    resultsLoc: 'LLM 调用 trace',
  },
};

/** settings 是全屏 overlay，其下 Stage 仍渲染 ChatView → 按 task 模式展示。 */
function modeForView(view: ViewId): ModeInfo {
  return MODE_BY_VIEW[view] ?? MODE_BY_VIEW.task;
}

export function PlanBar() {
  const activeView = useNavigationStore((s) => s.activeView);
  const project = useNavigationStore((s) => s.activeProject);
  const mode = modeForView(activeView);

  return (
    <header className={styles.planBar} data-testid="plan-bar">
      <div className={styles.planBarInner}>
        <span className={styles.modeBadge} data-testid="plan-mode">
          {mode.label}
        </span>
        <span className={styles.planMeta}>
          {mode.planLoc} · {mode.resultsLoc}
        </span>
        <span className={styles.planProject}>
          {project ? project.name : '未选项目'}
        </span>
        {/* 块2 填实：plan 大纲 + 预算/迭代 transparency（启示 3/7） */}
        <span className={styles.planPlaceholder}>plan · 预算 · 迭代 — 块2</span>
      </div>
    </header>
  );
}
