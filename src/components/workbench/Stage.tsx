import { useNavigationStore } from '../../stores/navigationStore';
import { ChatView } from '../chat/ChatView';
import { TraceView } from '../trace/TraceView';
import styles from './workbench.module.css';

/**
 * Stage — 轴线 B「编排在何处发生」（起底重构 块1 骨架）。
 *
 * 两视图：
 *  - Chat（task）：对话流。破坏性操作的审批由 ApprovalModal（Human Gate）承接。
 *  - Trace：可观测，LLM 调用 trace 树。
 *
 * 编排画布（手动连线 / agent 自规划 DAG）已整体移除；保留人工审批（Human Gate）。
 */
export function Stage() {
  const activeView = useNavigationStore((s) => s.activeView);

  return (
    <section className={styles.stage} data-testid="workbench-stage">
      {activeView === 'trace' ? <TraceView /> : <ChatView />}
    </section>
  );
}
