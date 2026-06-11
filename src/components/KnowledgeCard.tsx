import type { KnowledgeEntry } from '../types';

interface KnowledgeCardProps {
  entry: KnowledgeEntry;
  onDelete?: () => void;
}

export function KnowledgeCard({ entry, onDelete }: KnowledgeCardProps) {
  const confidence = Math.round(entry.confidence * 100);
  const time = new Date(entry.updatedAt).toLocaleDateString();

  return (
    <div className="knowledge-card">
      <div className="knowledge-card-header">
        <span className="knowledge-card-category">{entry.category}</span>
        <span className="knowledge-card-confidence">{confidence}%</span>
      </div>
      <h4 className="knowledge-card-title">{entry.title}</h4>
      <p className="knowledge-card-content">
        {entry.content.length > 200 ? entry.content.slice(0, 200) + '...' : entry.content}
      </p>
      <div className="knowledge-card-footer">
        <span className="knowledge-card-source">{entry.sourceAgent.replace(/_/g, ' ')}</span>
        <span className="knowledge-card-time">{time}</span>
        {onDelete && (
          <button className="knowledge-card-delete" onClick={onDelete} title="删除">×</button>
        )}
      </div>
    </div>
  );
}
