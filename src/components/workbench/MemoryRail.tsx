import { useEffect } from 'react';
import { useNavigationStore } from '../../stores/navigationStore';
import { useAgentStore } from '../../stores/agentStore';
import { useKnowledgeStore } from '../../stores/knowledgeStore';
import { GitPanel } from '../git/GitPanel';
import styles from './workbench.module.css';

/**
 * MemoryRail — 轴线 C「中间结果落在何处」（起底重构 块1 骨架 / 块5-5b 记忆可见）。
 *
 * 常驻右栏，承载「结果落点」相关的所有副信息：
 *
 *  - 记忆概览（块5）：当前会话的压缩次数 + 归档消息数（从 session.blocks 的 compact
 *    events 统计——死档归档已有，compaction-archive-complete）。常驻信号让用户感知
 *    context 压力，不必进对话流翻 CompactCard。卡片化（mr-card.archive 橙边）对齐
 *    原型 axis-workbench.html。
 *
 *  - 反思记录（块5b）：项目级 knowledge_entries 里 category=react_reflection 的条目。
 *    后端 persist_completion_memory 已落地（session_reflection.rs + knowledge/store.rs）：
 *    agent 完成会话时内核把结构化反思写入；executor memory_prompt_suffix 下次注入 system
 *    prompt。此处只读回展示——卡片化（mr-card.reflection 紫边 + 时间 meta）对齐原型。
 *
 *  - task 模式挂 GitPanel（文件变更 = 已落地的中间结果）
 *
 * 调研启示 4：记忆 = 反射缓冲（活，Reflexion）+ 分层归档（死，已有 compaction）双轨。
 */

/** 相对时间格式化（"2 分钟前"），用于反思/归档卡片 meta。空/非法 iso 返回空串。 */
function fmtRelative(iso: string): string {
  if (!iso) return '';
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return '';
  const diff = Math.max(0, Date.now() - then);
  const min = Math.floor(diff / 60000);
  if (min < 1) return '刚刚';
  if (min < 60) return `${min} 分钟前`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr} 小时前`;
  return `${Math.floor(hr / 24)} 天前`;
}

export function MemoryRail() {
  const activeView = useNavigationStore((s) => s.activeView);
  const activeProject = useNavigationStore((s) => s.activeProject);
  const conversationId = useNavigationStore((s) => s.selectedConversationId);
  const sessions = useAgentStore((s) => s.sessions);
  const entries = useKnowledgeStore((s) => s.entries);
  const loadForProject = useKnowledgeStore((s) => s.loadForProject);
  const isTask = activeView === 'task';

  // 记忆概览（块5）：当前会话所有 turn 的 compact events 汇总。
  const convSessions = conversationId
    ? sessions.filter((s) => s.conversationId === conversationId)
    : [];
  const compactEvents = convSessions.flatMap((s) =>
    (s.blocks ?? []).filter((b): b is Extract<typeof b, { kind: 'compact' }> => b.kind === 'compact'),
  );
  const compactCount = compactEvents.length;
  const droppedTotal = compactEvents.reduce((sum, e) => sum + e.dropped_count, 0);
  const lastCompactAt = compactEvents
    .map((e) => e.archived_at ?? '')
    .sort()
    .at(-1);

  // 反思记录（块5b）：项目切换时拉 knowledge，过滤 react_reflection 最近 5 条。
  useEffect(() => {
    if (activeProject) void loadForProject(activeProject.path);
  }, [activeProject, loadForProject]);

  const reflections = entries
    .filter((e) => e.category === 'react_reflection')
    .slice()
    .sort((a, b) => (b.createdAt || '').localeCompare(a.createdAt || ''))
    .slice(0, 5);

  return (
    <aside className={styles.memoryRail} data-testid="memory-rail">
      <div className={styles.railSection}>
        <h4 className={styles.railTitle}>记忆 · 结果落点</h4>
        {compactCount > 0 ? (
          <div
            className={`${styles.railCard} ${styles.railCardArchive}`}
            data-testid="memory-compaction-stat"
          >
            <div className={styles.railCardTitle}>
              压缩 {compactCount} 次 · 归档 {droppedTotal} 条消息
            </div>
            <div className={styles.railCardMeta}>
              compaction · {lastCompactAt ? `${fmtRelative(lastCompactAt)} · ` : ''}已存档可读回
            </div>
          </div>
        ) : (
          <div className={styles.railPlaceholder}>当前会话无压缩记录</div>
        )}
      </div>
      <div className={styles.railSection}>
        <h4 className={styles.railTitle}>反思记录 · 最近</h4>
        {reflections.length > 0 ? (
          <div className={styles.railReflectionList} data-testid="reflection-list">
            {reflections.map((r) => (
              <div
                key={r.id}
                className={`${styles.railCard} ${styles.railCardReflection}`}
                title={r.title || '(无标题)'}
              >
                <div className={styles.railCardTitle}>{r.title || '(无标题)'}</div>
                <div className={styles.railCardMeta}>
                  {fmtRelative(r.createdAt)} · 置信 {(r.confidence * 100).toFixed(0)}%
                </div>
              </div>
            ))}
          </div>
        ) : (
          <div className={styles.railPlaceholder} data-testid="reflection-placeholder">
            {activeProject ? '无反思记录——完成任务后内核自动积累' : '选择工作区后展示反思记录'}
          </div>
        )}
      </div>
      {isTask && (
        <div className={styles.railGitWrap}>
          <GitPanel projectPath={activeProject?.path ?? null} />
        </div>
      )}
    </aside>
  );
}
