import { Frame } from '../ui/Frame/Frame';
import styles from './UserMessage.module.css';

interface UserMessageProps {
  content: string;
}

/**
 * UserMessage — 用户消息卡。
 *
 * pi.dev 签名：default Frame 四角取景框 + 暖纸面板 + 衬线正文。
 * 之前的 .user-message / .user-message-bubble 散落 CSS 已收敛到 module。
 */
export function UserMessage({ content }: UserMessageProps) {
  return (
    <Frame variant="default" className={styles.wrap}>
      <div className={styles.meta}>
        <span className={styles.who}>USER ▸</span>
      </div>
      <div className={styles.body}>{content}</div>
    </Frame>
  );
}
