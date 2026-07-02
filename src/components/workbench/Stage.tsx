import { useNavigationStore } from '../../stores/navigationStore';
import { ChatView } from '../chat/ChatView';
import { OrchestrateView } from '../orchestrate/OrchestrateView';
import { TraceView } from '../trace/TraceView';
import styles from './workbench.module.css';

/**
 * Stage — 轴线 B「编排在何处发生」（起底重构 块1 骨架）。
 *
 * 按运行模式自适应渲染主内容区。骨架阶段直接按 activeView 路由现有三个视图
 * （chat / orchestrate / trace），收敛原先散落在 MainPanel 的路由逻辑。
 *
 * 后续块3 填实：三模式结构化呈现——
 *  - Chat 模式：对话流按 plan 步骤分组（不再纯时序压扁），BlockCard 加穷尽 default
 *    （修复现状静默丢未知事件的脆弱性）
 *  - DAG 模式：节点拓扑 + 运行时着色 + 节点内结构化输出
 *  - Self-Planning 模式：agent 自规划 DAG 动态生长拓扑（启示 8 orchestrator-workers）
 *  - Verify 节点：对抗式交叉验证作为 DAG 一等节点（启示 6）
 */
export function Stage() {
  const activeView = useNavigationStore((s) => s.activeView);

  return (
    <section className={styles.stage} data-testid="workbench-stage">
      {activeView === 'orchestrate' ? (
        <OrchestrateView />
      ) : activeView === 'trace' ? (
        <TraceView />
      ) : (
        <ChatView />
      )}
    </section>
  );
}
