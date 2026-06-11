import type { ParsedBlock } from '../utils/outputParser';

interface OutputBlockProps {
  block: ParsedBlock;
}

export function OutputBlock({ block }: OutputBlockProps) {
  switch (block.type) {
    case 'command':
      return <CommandBlock content={block.content} />;
    case 'diff':
      return <DiffBlock content={block.content} meta={block.meta} />;
    case 'filepath':
      return <FilepathBlock content={block.content} meta={block.meta} />;
    case 'progress':
      return <ProgressBlock content={block.content} meta={block.meta} />;
    case 'tool_tree':
      return <ToolTreeBlock content={block.content} />;
    case 'text':
    default:
      return <TextBlock content={block.content} />;
  }
}

function CommandBlock({ content }: { content: string }) {
  return (
    <div className="output-block output-command">
      <code>{content}</code>
    </div>
  );
}

function DiffBlock({ content, meta }: { content: string; meta?: ParsedBlock['meta'] }) {
  const lines = content.split('\n');
  return (
    <div className="output-block output-diff">
      {meta?.filePath && <div className="output-diff-file">{meta.filePath}</div>}
      <pre className="output-diff-content">
        {lines.map((line, i) => (
          <span
            key={i}
            className={
              line.startsWith('+') ? 'output-diff-add'
              : line.startsWith('-') ? 'output-diff-del'
              : line.startsWith('@@') ? 'output-diff-hunk'
              : ''
            }
          >
            {line + '\n'}
          </span>
        ))}
      </pre>
    </div>
  );
}

function FilepathBlock({ content, meta }: { content: string; meta?: ParsedBlock['meta'] }) {
  return (
    <div className="output-block output-filepath">
      <code>{meta?.filePath || content}</code>
    </div>
  );
}

function ProgressBlock({ content, meta }: { content: string; meta?: ParsedBlock['meta'] }) {
  const statusClass =
    meta?.status === 'done' ? 'progress-done'
    : meta?.status === 'failed' ? 'progress-failed'
    : 'progress-running';
  return (
    <div className={`output-block output-progress ${statusClass}`}>
      {content}
    </div>
  );
}

function ToolTreeBlock({ content }: { content: string }) {
  return (
    <div className="output-block output-tool-tree">
      <pre>{content}</pre>
    </div>
  );
}

function TextBlock({ content }: { content: string }) {
  if (!content.trim()) return null;
  return (
    <div className="output-block output-text">
      {content}
    </div>
  );
}
