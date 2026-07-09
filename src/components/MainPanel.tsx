import { WorkbenchSurface } from './workbench/WorkbenchSurface';

/**
 * Main stage — renders the axis-driven WorkbenchSurface（起底重构 块1）。
 *
 * 统一工作面（PlanBar / Stage / GateBar）。视图路由迁入 workbench/Stage。
 * 范式依据见 WorkbenchSurface.tsx 注释。
 */
export function MainStage() {
  return <WorkbenchSurface />;
}
