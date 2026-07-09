import { PlanBar } from './PlanBar';
import { Stage } from './Stage';
import { GateBar } from './GateBar';
import styles from './workbench.module.css';

/**
 * WorkbenchSurface — 轴线化工作面主容器（起底重构 块1 骨架）。
 *
 * 替换旧 chat/orchestrate/trace 三视图割裂路由，以三轴线 + 门控层重组主界面：
 *
 *   ┌─ PlanBar ────────────────────────────────┐  轴线A 谁持有 plan
 *   ├─ Stage ──────────────────────────────────┤  轴线B 编排所在 / 结果落点
 *   ├─ GateBar ────────────────────────────────┤  门控层 分层检测+成本
 *   └──────────────────────────────────────────┘
 *
 * 设计依据：deep-research 调研 8 条范式主张（见 memory/groundup-refactor-direction）。
 * 核心判断：下一代工作台不再"chat + 编排画布 + trace"割裂三件套，而以「谁持有 plan /
 * 编排在何处 / 中间结果落在何处」为一等 UI 轴线，把分层失败检测与成本预算做成前置门控。
 *
 * 根 <main> 占据 .app 网格的 main-stage 槽位（grid-area: main-stage），内部为 3 区网格。
 * 块1 仅立骨架：布局 + 路由收敛，各区实质内容由后续块填实（见各组件注释）。
 */
export function WorkbenchSurface() {
  return (
    <main className={styles.workbench} data-testid="workbench-surface">
      <PlanBar />
      <Stage />
      <GateBar />
    </main>
  );
}
