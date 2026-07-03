import { useNavigationStore } from '../../stores/navigationStore';
import { ChatView } from '../chat/ChatView';
import { TraceView } from '../trace/TraceView';
import styles from './workbench.module.css';

/**
 * Stage — 轴线 B「编排在何处发生」（起底重构 块1 骨架）。
 *
 * 砍 DAG 手动画布编排后剩两视图：
 *  - Chat（task）：对话流。agent 自规划 DAG 由 WorkflowTool 在消息内触发，进度经
 *    WorkflowProgressStrip 实时呈现，节点结果落在 tool_result pill——不再需要独立画布。
 *  - Trace：可观测，LLM 调用 trace 树。
 *
 * 用户手动连线编排的 OrchestrateView 已移除（决定：砍 DAG 编排画布，保留 agent 自规划
 * + 人工审批）。agent 自动化编排与用户修改能力仍完整：WorkflowTool 自规划执行 +
 * ApprovalModal 在破坏性操作时承接审批。
 */
export function Stage() {
  const activeView = useNavigationStore((s) => s.activeView);

  return (
    <section className={styles.stage} data-testid="workbench-stage">
      {activeView === 'trace' ? <TraceView /> : <ChatView />}
    </section>
  );
}
