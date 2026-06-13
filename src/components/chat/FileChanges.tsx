import { useMemo } from 'react';
import type { Session, ContextSnapshot } from '../../types';
import { IconEdit } from '../Icons';

interface FileChangesProps {
  session: Session | null;
}

interface FileChange {
  path: string;
  added: number;
  removed: number;
}

function extractFileChanges(snapshot: ContextSnapshot | null | undefined): FileChange[] {
  if (!snapshot || !snapshot.filesChanged) return [];
  return snapshot.filesChanged.map(path => ({
    path,
    added: 0,
    removed: 0,
  }));
}

export function FileChanges({ session }: FileChangesProps) {
  const files = useMemo(() => extractFileChanges(session?.contextSnapshot), [session?.contextSnapshot]);

  if (files.length === 0) return null;

  return (
    <div className="agent-block">
      <div className="agent-block-header">
        <span className="agent-block-title">File Changes</span>
        <span className="agent-block-badge">{files.length} files</span>
      </div>
      <div className="agent-block-body">
        <div className="file-changes-list">
          {files.map((file, i) => (
            <div key={i} className="file-change-item">
              <span className="file-change-icon"><IconEdit size={14} /></span>
              <span className="file-change-path">{file.path}</span>
              {(file.added > 0 || file.removed > 0) && (
                <span className="file-change-stats">
                  {file.added > 0 && <span className="file-change-added">+{file.added}</span>}
                  {file.removed > 0 && <span className="file-change-removed">-{file.removed}</span>}
                </span>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
