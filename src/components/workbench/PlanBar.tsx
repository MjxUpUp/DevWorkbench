import { useNavigationStore, type ViewId } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';
import type { SessionStatus } from '../../types';
import styles from './workbench.module.css';

/**
 * PlanBar — 轴线 A「谁持有 plan」（起底重构 块1 骨架 / 块2 进度可见）。
 *
 * 顶栏承载：运行模式切换（mode-segmented）+ plan/结果物理落点 + plan 执行进度（轻量
 * 文本）+ 当前工作区。
 *
 * 编排画布移除后，运行模式只剩 Chat（plan ∈ LLM context 隐式）与 Trace（可观测）。
 *
 * 设计修正（B1）：不在顶栏渲染 tool 调用序列。chat 模式 plan ∈ LLM context 隐式，前端
 * 拿不到真正的 plan 阶段；旧 planStepper 用 tool_use 序列当「代理」，只是把 Stage 已有
 * 的 tool 调用噪音化堆到顶栏——重复且偏离「plan 阶段」语义。进度只保留汇总文本（步骤
 * N · 状态），tool 详情归 Stage（BlocksView 步骤分组）。
 *
 * 成本预算不在本栏：per-session cost 不可得 → 归 GateBar（块4）。
 */
type ModeInfo = {
  /** plan 当前由谁持有 */
  planLoc: string;
  /** 中间结果落在何处 */
  resultsLoc: string;
};

const MODE_BY_VIEW: Record<Exclude<ViewId, 'search' | 'settings'>, ModeInfo> = {
  task: {
    planLoc: 'plan ∈ LLM context',
    resultsLoc: 'results ∈ 对话历史',
  },
  trace: {
    planLoc: '—',
    resultsLoc: 'LLM 调用 trace',
  },
};

/** mode-segmented 两模式（plan 相关轴线）；search/settings 非运行模式不进切换器。 */
const MODE_SEGMENTS: { id: 'task' | 'trace'; label: string }[] = [
  { id: 'task', label: 'Chat' },
  { id: 'trace', label: 'Trace' },
];

/** settings 是全屏 overlay（其下 Stage 仍渲染 ChatView）、search 是独立检索——皆按 task 展示。 */
function modeForView(view: ViewId): ModeInfo {
  return view === 'trace' ? MODE_BY_VIEW.trace : MODE_BY_VIEW.task;
}

const STATUS_ZH: Record<SessionStatus, string> = {
  running: '运行中',
  completed: '完成',
  failed: '失败',
  cancelled: '取消',
};

export function PlanBar() {
  const activeView = useNavigationStore((s) => s.activeView);
  const setActiveView = useNavigationStore((s) => s.setActiveView);
  const project = useNavigationStore((s) => s.activeProject);
  const conversationId = useNavigationStore((s) => s.selectedConversationId);
  const sessions = useAgentStore((s) => s.sessions);
  const mode = modeForView(activeView);

  // plan 进度派生（轴线A 的「执行可见」补充「位置可见」）。
  const convTurns =
    activeView === 'task' && conversationId
      ? sessions.filter((s) => s.conversationId === conversationId)
      : [];
  // running 优先（正在跑的 turn 即当前 plan）；否则取 startedAt 最新。
  const current =
    convTurns.find((s) => s.status === 'running') ??
    [...convTurns].sort((a, b) => (a.startedAt < b.startedAt ? 1 : -1))[0];

  const steps = current?.blocks?.filter((b) => b.kind === 'tool_use').length ?? 0;
  const progress =
    activeView === 'task'
      ? current
        ? `步骤 ${steps} · ${STATUS_ZH[current.status]}`
        : '无活跃会话'
      : '—';

  return (
    <header className={styles.planBar} data-testid="plan-bar">
      <div className={styles.planBarInner}>
        <div className={styles.modeSegmented} role="group" aria-label="运行模式">
          {MODE_SEGMENTS.map((seg) => {
            const active = activeView === seg.id;
            return (
              <button
                key={seg.id}
                type="button"
                aria-pressed={active}
                data-testid={active ? 'plan-mode' : `mode-seg-${seg.id}`}
                className={`${styles.modeBtn} ${active ? styles.modeBtnActive : ''}`}
                onClick={() => setActiveView(seg.id)}
              >
                {seg.label}
              </button>
            );
          })}
        </div>
        <span className={styles.planMeta}>
          {mode.planLoc} · {mode.resultsLoc}
        </span>
        <span className={styles.planProgress} data-testid="plan-progress">
          {progress}
        </span>
        <span className={styles.planProject}>
          {project ? project.name : '未选工作区'}
        </span>
      </div>
    </header>
  );
}
