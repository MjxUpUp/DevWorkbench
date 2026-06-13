interface UserMessageProps {
  content: string;
}

export function UserMessage({ content }: UserMessageProps) {
  return (
    <div className="user-message">
      <div className="user-message-content">{content}</div>
    </div>
  );
}
