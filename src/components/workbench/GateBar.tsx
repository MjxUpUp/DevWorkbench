import { useAgentStore } from '../../stores/agentStore';
import styles from './workbench.module.css';

/**
 * GateBar — 门控层（起底重构 块1 骨架）。
 *
 * 常驻底栏，承载分层失败检测 + 成本治理（前置门控，非事后 trace）。骨架阶段仅显示
 * 实时运行态（agentStore 中 status==='running' 的会话数），其余门控占位。
 *
 * 调研启示 2/3（分层检测 + 成本前置门控）：
 *  - pre-action：破坏性操作 human-gate（已落 CatastropheGuard，保留）
 *  - during-action：任意时刻 interrupt/resume 控制流原语（架构级，非仅按钮）
 *  - multi-step：预算/迭代上限熔断 + step-repetition 检测（直击 MAST 17.14% 最大失败）
 * 块4 填实三层状态可视化 + 预算条 + 熔断指示。
 *
 * 反例（启示 2）：不把每个微操作都弹审批（automation bias/skill fade 反而降低干预
 * 有效性）；不把人审当唯一安全网（结构性不可扩展，PAI claim3/4）。
 */
export function GateBar() {
  // 选 sessions 数组（不是单个字段）以在状态变化时重渲染；运行计数由此派生。
  const sessions = useAgentStore((s) => s.sessions);
  const runningCount = sessions.filter((s) => s.status === 'running').length;
  const isRunning = runningCount > 0;

  return (
    <footer className={styles.gateBar} data-testid="gate-bar">
      <span className={styles.gateStatus} data-running={isRunning}>
        <span className={styles.gateDot} aria-hidden="true" />
        {isRunning ? `${runningCount} 个 agent 运行中` : 'idle'}
      </span>
      {/* 块4 填实：审批队列 · interrupt/resume · 预算条 + 熔断（启示 2/3） */}
      <span className={styles.gatePlaceholder}>
        分层检测 · 预算条 — 块4
      </span>
    </footer>
  );
}
