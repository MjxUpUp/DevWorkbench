import { WorkbenchSurface } from './workbench/WorkbenchSurface';

/**
 * Main stage — renders the axis-driven WorkbenchSurface（起底重构 块1）。
 *
 * 旧 chat/orchestrate/trace 三视图路由 + task-mode GitPanel 2-col 布局已收敛进统一的
 * 4 区工作面（PlanBar / Stage / MemoryRail / GateBar）。视图路由迁入 workbench/Stage，
 * GitPanel 迁入 workbench/MemoryRail。范式依据见 WorkbenchSurface.tsx 注释。
 */
export function MainStage() {
  return <WorkbenchSurface />;
}
