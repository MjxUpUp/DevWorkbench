/**
 * 设置页分区 id — 设置页内部路由 + 外部直达入口（命令面板「技能」）的共用类型。
 *
 * 独立成文件，让 navigationStore（store 层）能 type-only import 此类型，而不引入对
 * SettingsView（组件层）的运行时循环依赖（store → 组件）。type-only 在编译期擦除，
 * 运行时 store 不依赖任何组件模块。
 *
 * 唯一权威源：新增分区时此处与 SettingsView 的 SECTIONS 数组同步——SectionDef.id
 * 标注为 SettingsSection 会反向强制 SECTIONS 的 id 必须是此处声明的成员，漏改即
 * 编译报错。
 */
export type SettingsSection =
  | 'agent-tools' | 'providers' | 'capability'
  | 'skills' | 'mcp' | 'sub-agents' | 'commands' | 'hooks'
  | 'memory' | 'output-style' | 'usage-stats' | 'trace' | 'onboarding';
