import { WorkbenchSurface } from './workbench/WorkbenchSurface';

/**
 * Main stage — renders the axis-driven WorkbenchSurface（起底重构 块1）。
 *
 * 旧 chat/orchestrate/trace 三视图路由已收敛进统一的工作面
 * （PlanBar / Stage / GateBar）。视图路由迁入 workbench/Stage。范式依据见
 * WorkbenchSurface.tsx 注释。
 */
export function MainStage() {
  return <WorkbenchSurface />;
}
