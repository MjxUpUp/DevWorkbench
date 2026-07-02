import { useNavigationStore } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';
import { GitPanel } from '../git/GitPanel';
import styles from './workbench.module.css';

/**
 * MemoryRail — 轴线 C「中间结果落在何处」（起底重构 块1 骨架 / 块5 记忆可见）。
 *
 * 常驻右栏，承载「结果落点」相关的所有副信息。块5 把占位换成真实记忆可见性：
 *  - 记忆概览：当前会话的压缩次数 + 归档消息数（从 session.blocks 的 compact events
 *    统计——死档归档已有，compaction-archive-complete）。这是「记忆已被压缩多少」的
 *    常驻信号，让用户感知 context 压力，而不必进对话流翻 CompactCard。
 *  - 反射笔记（占位）：Reflexion 式活记忆——agent 失败/纠错时的 verbal 反思，可被
 *    下次试验读回（启示4）。需后端 reflection buffer（存储 + read IPC + ReactAgent
 *    读回注入），留后端阶段；前端先立占位标注依赖。
 *  - task 模式挂 GitPanel（文件变更 = 已落地的中间结果）
 *
 * 调研启示 4：记忆 = 反射缓冲（活，Reflexion）+ 分层归档（死，已有 compaction）双轨。
 * 适合桌面工作台：本地可持久化大体积原文归档（web 受限）。反例：不把记忆等同于全文
 * 压缩（那是死档不是可推理的反射）。
 */
export function MemoryRail() {
  const activeView = useNavigationStore((s) => s.activeView);
  const activeProject = useNavigationStore((s) => s.activeProject);
  const conversationId = useNavigationStore((s) => s.selectedConversationId);
  const sessions = useAgentStore((s) => s.sessions);
  const isTask = activeView === 'task';

  // 记忆概览：当前会话所有 turn 的 compact events 汇总（压缩次数 + 归档消息数）。
  const convSessions = conversationId
    ? sessions.filter((s) => s.conversationId === conversationId)
    : [];
  const compactEvents = convSessions.flatMap((s) =>
    (s.blocks ?? []).filter((b): b is Extract<typeof b, { kind: 'compact' }> => b.kind === 'compact'),
  );
  const compactCount = compactEvents.length;
  const droppedTotal = compactEvents.reduce((sum, e) => sum + e.dropped_count, 0);

  return (
    <aside className={styles.memoryRail} data-testid="memory-rail">
      <div className={styles.railSection}>
        <h4 className={styles.railTitle}>记忆 · 结果落点</h4>
        {compactCount > 0 ? (
          <div className={styles.railStat} data-testid="memory-compaction-stat">
            压缩 {compactCount} 次 · 归档 {droppedTotal} 条消息
          </div>
        ) : (
          <div className={styles.railPlaceholder}>当前会话无压缩记录</div>
        )}
      </div>
      <div className={styles.railSection}>
        <h4 className={styles.railTitle}>反射笔记</h4>
        {/* 启示4 Reflexion：活记忆，需后端 reflection buffer（write 失败反思 + read
            注入下次试验）。前端先立占位，后端实现后此处列反思条目。 */}
        <div className={styles.railPlaceholder} data-testid="reflection-placeholder">
          反射笔记 — 待后端 reflection buffer
        </div>
      </div>
      {isTask && (
        <div className={styles.railGitWrap}>
          <GitPanel projectPath={activeProject?.path ?? null} />
        </div>
      )}
    </aside>
  );
}
