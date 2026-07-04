import { useEffect } from 'react';
import { useAgentStore } from '../../stores/agentStore';
import { useNavigationStore } from '../../stores/navigationStore';
import type { Project } from '../../types';
import styles from './ConversationBookmarks.module.css';

/**
 * ConversationBookmarks — 工作区顶部常驻会话书签栏（用户决定：旧会话以书签方式
 * 平铺，每个旧会话一个书签，可新增可删除；替代旧的「两张卡片 + 列表」入口）。
 *
 * ModelSelector 下沉到 Composer 后，ChatHeader 让位给本组件，常驻 ChatView 顶部
 * （empty state + active 对话态都渲染），所以用户在任何会话里都能切换 / 新增 /
 * 删除会话——这是「书签」心智的关键（仅空态显示会断掉对话中的切换能力）。
 *
 * 数据/IPC 全就绪（list/delete_conversation + selectConversation）；本组件是
 * deleteConversation action 的第一个 UI 调用方（之前只有 store action + 测试，
 * 无组件接线）。删除当前选中会话时先 selectConversation(null) 回空态，避免停在
 * 已删会话上。
 */
interface Props {
  project: Project;
  /** 当前 turn 的 session id（运行态/已选会话），mono 小标签显示（Cursor 范式）。 */
  requestId?: string;
  running?: boolean;
}

export function ConversationBookmarks({ project, requestId, running }: Props) {
  const allConversations = useAgentStore((s) => s.conversations);
  const refreshConversations = useAgentStore((s) => s.refreshConversations);
  const deleteConversation = useAgentStore((s) => s.deleteConversation);
  const activeConversationId = useNavigationStore((s) => s.selectedConversationId);
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

  const handleDelete = async (id: string) => {
    // 删的是当前选中会话 → 先切回空态，避免停在已删会话上（turns 立即清空）。
    const wasActive = activeConversationId === id;
    if (wasActive) selectConversation(null);
    try {
      await deleteConversation(id, project.path);
    } catch (e) {
      // IPC 失败：会话实际未删。回滚选中（若刚切了空态），下次 refresh 会恢复书签。
      console.error('delete_conversation failed', e);
      if (wasActive) selectConversation(id);
    }
  };

  return (
    <div className={styles.bookmarks} data-testid="conversation-bookmarks">
      <div className={styles.list} role="tablist" aria-label="会话书签">
        {conversations.map((c) => {
          const active = c.id === activeConversationId;
          return (
            <div key={c.id} className={`${styles.bookmark}${active ? ` ${styles.active}` : ''}`}>
              <button
                type="button"
                role="tab"
                aria-selected={active}
                className={styles.bookmarkBtn}
                title={c.title}
                onClick={() => selectConversation(c.id)}
              >
                {c.pinned && <span className={styles.pin} aria-hidden="true">📌</span>}
                <span className={styles.title}>{c.title}</span>
              </button>
              <button
                type="button"
                className={styles.close}
                title="删除会话"
                aria-label={`删除会话 ${c.title}`}
                onClick={() => void handleDelete(c.id)}
              >
                ×
              </button>
            </div>
          );
        })}
        <button
          type="button"
          className={styles.add}
          title="开始新会话（保留当前输入框内容）"
          aria-label="开始新会话（保留当前输入）"
          data-testid="conversation-bookmark-add"
          onClick={() => selectConversation(null)}
        >
          + 新建
        </button>
      </div>
      {(requestId || running) && (
        <span className={styles.meta}>
          {running && <span className={styles.livedot} aria-hidden="true" />}
          {requestId && <span className={styles.requestId}>{requestId}</span>}
        </span>
      )}
    </div>
  );
}
