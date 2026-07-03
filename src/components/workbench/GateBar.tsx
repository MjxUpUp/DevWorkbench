import { useEffect } from 'react';
import { useAgentStore } from '../../stores/agentStore';
import { useDashboardStore } from '../../stores/dashboardStore';
import styles from './workbench.module.css';

/**
 * GateBar — 门控层（起底重构 块1 骨架 / 块4 成本门控）。
 *
 * 常驻底栏，承载分层失败检测 + 成本治理（前置门控，非事后 trace）。
 *
 * 块4 填实（调研启示 3 成本前置门控）：
 *  - 累计成本 + 月度预算进度条（dashboardStore.budget/costSummary，全局聚合——
 *    per-session cost 不可得故用全局，与 PlanBar 分工：PlanBar=plan 可见 / GateBar=成本门控）
 *  - 超预算熔断警告（percentage>=100 → 红色 + ⚠），让成本失控前置可见
 *  - 挂载即 fetchDashboard：门控层常驻，成本必须实时可见而非等用户开 Dashboard
 *
 * 已落地（G3，react_agent.rs 后端强制）：
 *  - step-repetition 硬熔断：同一 tool+args 连续重复到阈值即停（直击 MAST 17.14%
 *    最大失败）。run_loop（子agent路径）+ 流式 run()（chat主路径）双接入，trip→
 *    Failed + output_summary 注明「step 重复熔断」。前端不在此常驻 badge——它是
 *    后端隐式保护，触发时以 Failed 终态浮现，不属 GateBar 实时状态范畴。
 *
 * 未做（仍 pending / 块4b）：
 *  - during-action interrupt：ChatView 已有 stopAgent 按钮（stop_agent_session IPC），
 *    GateBar 不重复造；与成本熔断联动的硬停止属后端 cost breaker，前端先做软警告
 *
 * 反例（启示 2）：不把每个微操作都弹审批（automation bias/skill fade）；不把人审当
 * 唯一安全网（结构性不可扩展，PAI claim3/4）。
 */
export function GateBar() {
  // 选 sessions 数组以在状态变化时重渲染；运行计数由此派生。
  const sessions = useAgentStore((s) => s.sessions);
  const fetchDashboard = useDashboardStore((s) => s.fetchDashboard);
  const budget = useDashboardStore((s) => s.budget);
  const costSummary = useDashboardStore((s) => s.costSummary);

  const runningCount = sessions.filter((s) => s.status === 'running').length;
  const isRunning = runningCount > 0;
  const overBudget = budget.total > 0 && budget.percentage >= 100;
  // G2 三态：正常 / near（≥80% 黄警告，成本接近失控前置可见）/ over（≥100% 红熔断）。
  const nearBudget = budget.total > 0 && !overBudget && budget.percentage >= 80;

  // 门控层常驻——挂载即拉预算/成本，让成本前置可见（启示3：非事后 trace）。
  useEffect(() => {
    void fetchDashboard();
  }, [fetchDashboard]);

  return (
    <footer className={styles.gateBar} data-testid="gate-bar">
      <span className={styles.gateStatus} data-running={isRunning}>
        <span className={styles.gateDot} aria-hidden="true" />
        {isRunning ? `${runningCount} 个 agent 运行中` : 'idle'}
      </span>
      {costSummary && (
        <span className={styles.gateCost} data-testid="gate-cost">
          累计 ${costSummary.totalCost.toFixed(2)}
        </span>
      )}
      {budget.total > 0 && (
        <span className={styles.gateBudget} data-over={overBudget} data-near={nearBudget} data-testid="gate-budget">
          <span className={styles.gateBudgetText}>
            预算 ${budget.spent.toFixed(2)} / ${budget.total.toFixed(2)}
          </span>
          <span className={styles.gateBudgetBar} data-testid="gate-budget-bar">
            <span
              className={styles.gateBudgetFill}
              style={{ width: `${Math.min(100, budget.percentage)}%` }}
            />
          </span>
          {overBudget && <span className={styles.gateBreaker}>⚠ 超预算</span>}
          {nearBudget && <span className={styles.gateBreakerNear}>⚠ 接近预算</span>}
        </span>
      )}
      {/* 块4b/后端：during-action interrupt（成本熔断硬停止，需后端 cost breaker）。
          step-重复熔断已落地（G3 react_agent.rs），不在此占位——见文件头注释。 */}
      <span className={styles.gatePlaceholder}>during-action 成本中断 — 待后端 cost breaker</span>
    </footer>
  );
}
