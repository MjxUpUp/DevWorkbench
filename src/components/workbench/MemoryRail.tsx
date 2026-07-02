import { useNavigationStore } from '../../stores/navigationStore';
import { GitPanel } from '../git/GitPanel';
import styles from './workbench.module.css';

/**
 * MemoryRail — 轴线 C「中间结果落在何处」（起底重构 块1 骨架）。
 *
 * 常驻右栏，承载"结果落点"相关的所有副信息。骨架阶段：
 *  - 顶部"记忆双轨"占位（块5 填实：归档原文 / 压缩摘要 / 反射笔记 三类视图）
 *  - task 模式下挂 GitPanel（文件变更 = 已落地的中间结果），保留现有功能
 *
 * 调研启示 4：记忆 = 反射缓冲（活，Reflexion）+ 分层归档（死，已有 compaction）双轨。
 * 项目已落 compaction + 原文归档（死档），块5 补反射笔记（活，agent 失败/纠错时的
 * verbal 反思，可被下次试验读回）。UI 必须区分三类，不能混为一谈。
 *
 * 适合桌面工作台：本地可持久化大体积原文归档（web 受限）。反例：不把记忆等同于
 * 全文压缩（那是死档不是可推理的反射）。
 */
export function MemoryRail() {
  const activeView = useNavigationStore((s) => s.activeView);
  const activeProject = useNavigationStore((s) => s.activeProject);
  const isTask = activeView === 'task';

  return (
    <aside className={styles.memoryRail} data-testid="memory-rail">
      <div className={styles.railSection}>
        <h4 className={styles.railTitle}>记忆 · 结果落点</h4>
        {/* 块5 填实：归档原文(回放) / 压缩摘要(省 ctx) / 反射笔记(指导重试) */}
        <div className={styles.railPlaceholder}>三类记忆视图 — 块5</div>
      </div>
      {isTask && (
        <div className={styles.railGitWrap}>
          <GitPanel projectPath={activeProject?.path ?? null} />
        </div>
      )}
    </aside>
  );
}
