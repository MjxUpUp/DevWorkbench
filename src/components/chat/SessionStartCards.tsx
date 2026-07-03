import { useState, useEffect } from 'react';
import { useAgentStore } from '../../stores/agentStore';
import { useNavigationStore } from '../../stores/navigationStore';
import type { Project } from '../../types';
import styles from './SessionStartCards.module.css';

/**
 * SessionStartCards — 工作区顶部卡片式会话入口（用户决定：抛弃侧栏切会话，每个工作区
 * 顶部卡片选择「开始新会话」/「选择旧会话」，Excel-sheet 式在工作区下管理会话）。
 *
 * 在 ChatView 的 empty state（工作区已选 + 无选中会话 + 无运行 turn）渲染，替代旧的
 * 「创建任务」纯文本提示。两张卡片 tab：
 *  - 开始新会话（默认 active）：下方 Composer 即输入入口，卡片只是切换器
 *  - 选择旧会话：展开该工作区的会话列表，点一条 → selectConversation → 进对话态
 *
 * 旧会话列表复用 Sidebar.ConversationList 的数据逻辑（refreshConversations + 按
 * projectPath 过滤 + pinned 优先 / 最近活动排序），承载在工作区顶部而非侧栏。
 */
export function SessionStartCards({ project }: { project: Project }) {
  const [mode, setMode] = useState<'new' | 'old'>('new');
  const allConversations = useAgentStore((s) => s.conversations);
  const refreshConversations = useAgentStore((s) => s.refreshConversations);
  const selectConversation = useNavigationStore((s) => s.selectConversation);

  useEffect(() => {
    void refreshConversations(project.path);
  }, [project.path, refreshConversations]);

  const conversations = allConversations
    .filter((c) => c.projectPath === project.path)
    .sort((a, b) => {
      if (a.pinned !== b.pinned) return a.pinned ? -1 : 1;
      return new Date(b.lastActivityAt).getTime() - new Date(a.lastActivityAt).getTime();
    });

  return (
    <div className={styles.wrap} data-testid="session-start-cards">
      <div className={styles.cards} role="tablist" aria-label="会话入口">
        <button
          type="button"
          role="tab"
          aria-selected={mode === 'new'}
          className={`${styles.card} ${mode === 'new' ? styles.cardActive : ''}`}
          onClick={() => setMode('new')}
          data-testid="session-card-new"
        >
          <span className={styles.cardIcon} aria-hidden="true">✦</span>
          <span className={styles.cardTitle}>开始新会话</span>
          <span className={styles.cardDesc}>在下方输入需求，与 Kernel Agent 协作</span>
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={mode === 'old'}
          className={`${styles.card} ${mode === 'old' ? styles.cardActive : ''}`}
          onClick={() => setMode('old')}
          data-testid="session-card-old"
        >
          <span className={styles.cardIcon} aria-hidden="true">↻</span>
          <span className={styles.cardTitle}>选择旧会话</span>
          <span className={styles.cardDesc}>
            {conversations.length > 0 ? `${conversations.length} 个历史会话` : '暂无历史会话'}
          </span>
        </button>
      </div>

      {mode === 'new' ? (
        <div className={styles.newHint} data-testid="session-new-hint">
          在下方输入需求或指令，开始与 Kernel Agent 协作
        </div>
      ) : (
        <div className={styles.convList} data-testid="session-old-list">
          {conversations.length === 0 ? (
            <div className={styles.convEmpty}>该工作区暂无历史会话</div>
          ) : (
            conversations.map((c) => (
              <button
                key={c.id}
                type="button"
                className={styles.convItem}
                onClick={() => selectConversation(c.id)}
                title={c.title}
              >
                {c.pinned && <span className={styles.pin} aria-hidden="true">📌</span>}
                <span className={styles.convTitle}>{c.title}</span>
                {c.lastAgent && <span className={styles.convMeta}>{c.lastAgent}</span>}
              </button>
            ))
          )}
        </div>
      )}
    </div>
  );
}
