import { Fragment } from 'react';
import { useNavigationStore, type ViewId } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';
import { useOrchestrateStore } from '../../stores/orchestrateStore';
import type { Session, SessionStatus } from '../../types';
import styles from './workbench.module.css';

/**
 * PlanBar — 轴线 A「谁持有 plan」（起底重构 块1 骨架 / 块2 进度可见）。
 *
 * 调研启示 1/5/7/8：把"谁持有 plan / 编排在何处 / 中间结果落在何处"做成 UI 一等
 * 轴线。骨架阶段（块1）：按当前视图派生运行模式 + plan/结果物理落点。块2：补 plan
 * 执行进度可见性——让用户一眼看到 plan 走到哪了，而不必翻对话流。
 *
 * 借鉴：Claude Code "who holds the plan" 设计轴（claim18，3-0 验证）+ Anthropic
 * workflow-vs-agent 二分（claim11）。反例：不默认所有任务走 chat agent loop
 * （4-15x 成本膨胀，claim2）。
 *
 * 进度数据源（块2 关键决策）：
 *  - Chat 模式：plan ∈ LLM context（隐式），无显式 plan 对象 → 以「已执行 tool 步骤数 +
 *    session 状态」作为 plan 推进度的代理信号。取选中 conversation 里最新 turn
 *    （running 优先，否则 startedAt 最新；sessions 顺序不保证故显式排序）。
 *  - DAG 模式：plan ∈ 脚本变量（显式 GraphDef）→ orchestrateStore.nodes 即 plan 节点，
 *    done/total 直接得。
 *  - 成本预算不在本栏：per-session cost 不可得（Session 无字段）→ 归属 GateBar（块4）。
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

const STATUS_ZH: Record<SessionStatus, string> = {
  running: '运行中',
  completed: '完成',
  failed: '失败',
  cancelled: '取消',
};

type ToolStepStatus = 'done' | 'active' | 'error';
interface ToolStep {
  name: string;
  status: ToolStepStatus;
}

/** B1：从 session.blocks 派生 tool 步骤（与 BlocksView.groupByStep 同语义——tool_use
 *  起步，配对 tool_result 标记 done/error，否则 active）。Chat 模式 plan∈LLM context
 *  隐式，前端只看得见已发起的 tool 调用：done/active 是事后/进行中信号，未来步骤不
 *  在其中（stepper 末尾标注「未来∈LLM context」诚实告知边界，不造 pending 假步骤）。 */
function deriveToolSteps(blocks: Session['blocks']): ToolStep[] {
  if (!blocks || blocks.length === 0) return [];
  const steps: ToolStep[] = [];
  for (const b of blocks) {
    if (b.kind === 'tool_use') {
      steps.push({ name: b.name, status: 'active' });
    } else if (b.kind === 'tool_result' && steps.length > 0) {
      const last = steps[steps.length - 1];
      if (last.status === 'active') {
        last.status = b.is_error ? 'error' : 'done';
      }
    }
  }
  return steps;
}

export function PlanBar() {
  const activeView = useNavigationStore((s) => s.activeView);
  const project = useNavigationStore((s) => s.activeProject);
  const conversationId = useNavigationStore((s) => s.selectedConversationId);
  const sessions = useAgentStore((s) => s.sessions);
  const nodes = useOrchestrateStore((s) => s.nodes);
  const mode = modeForView(activeView);

  // plan 进度派生（轴线A 的「执行可见」补充「位置可见」）。
  // current 提到顶层：B1 stepper 也需要 current.blocks 派生 tool 步骤。
  const convTurns =
    activeView === 'task' && conversationId
      ? sessions.filter((s) => s.conversationId === conversationId)
      : [];
  // running 优先（正在跑的 turn 即当前 plan）；否则取 startedAt 最新。
  const current =
    convTurns.find((s) => s.status === 'running') ??
    [...convTurns].sort((a, b) => (a.startedAt < b.startedAt ? 1 : -1))[0];
  // B1：tool 步骤可视化（done/active dots），仅 task 视图有 current session 时渲染。
  const toolSteps = current ? deriveToolSteps(current.blocks) : [];

  let progress: string;
  if (activeView === 'task') {
    const steps = current?.blocks?.filter((b) => b.kind === 'tool_use').length ?? 0;
    progress = current
      ? `步骤 ${steps} · ${STATUS_ZH[current.status]}`
      : '无活跃会话';
  } else if (activeView === 'orchestrate') {
    const vals = Object.values(nodes);
    progress =
      vals.length === 0
        ? '未加载工作流'
        : `节点 ${vals.filter((n) => n.status === 'done').length}/${vals.length}` +
          (vals.some((n) => n.status === 'running') ? ' · 运行中' : '');
  } else {
    progress = '—';
  }

  return (
    <header className={styles.planBar} data-testid="plan-bar">
      <div className={styles.planBarInner}>
        <span className={styles.modeBadge} data-testid="plan-mode">
          {mode.label}
        </span>
        <span className={styles.planMeta}>
          {mode.planLoc} · {mode.resultsLoc}
        </span>
        {toolSteps.length > 0 && (
          <div
            className={styles.planStepper}
            data-testid="plan-stepper"
            aria-label={`plan 进度 ${progress}`}
          >
            {toolSteps.map((s, i) => (
              <Fragment key={i}>
                <span
                  className={`${styles.step} ${
                    s.status === 'done'
                      ? styles.stepDone
                      : s.status === 'active'
                        ? styles.stepActive
                        : styles.stepError
                  }`}
                  title={s.name}
                >
                  <span className={styles.stepDot} aria-hidden="true">
                    {s.status === 'done' ? '✓' : s.status === 'error' ? '✗' : i + 1}
                  </span>
                  <span className={styles.stepName}>{s.name}</span>
                </span>
                {i < toolSteps.length - 1 && (
                  <span
                    className={`${styles.stepLine} ${
                      s.status === 'done' ? styles.stepLineDone : ''
                    }`}
                  />
                )}
              </Fragment>
            ))}
            <span
              className={styles.stepFuture}
              title="Chat 模式 plan ∈ LLM context，未来步骤对前端不可见"
            >
              未来 ∈ LLM
            </span>
          </div>
        )}
        <span className={styles.planProgress} data-testid="plan-progress">
          {progress}
        </span>
        <span className={styles.planProject}>
          {project ? project.name : '未选项目'}
        </span>
      </div>
    </header>
  );
}
