import { useState, useEffect, useRef } from 'react';
import { listen } from '@tauri-apps/api/event';

interface AgentOutputProps {
  sessionId: string | null;
}

interface OutputLine {
  id: number;
  text: string;
}

export function AgentOutput({ sessionId }: AgentOutputProps) {
  const [lines, setLines] = useState<OutputLine[]>([]);
  const lineCounter = useRef(0);
  const bottomRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setLines([]);
    lineCounter.current = 0;

    if (!sessionId) return;

    const unlisten = listen<{ sessionId: string; text: string }>('agent:output', (event) => {
      if (event.payload.sessionId === sessionId) {
        const id = ++lineCounter.current;
        setLines(prev => [...prev, { id, text: event.payload.text }]);
      }
    });

    return () => {
      unlisten.then(fn => fn());
    };
  }, [sessionId]);

  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: 'smooth' });
  }, [lines]);

  if (!sessionId) {
    return (
      <div className="agent-output agent-output-empty">
        <span className="agent-output-placeholder">等待输出...</span>
      </div>
    );
  }

  if (lines.length === 0) {
    return (
      <div className="agent-output agent-output-empty">
        <span className="agent-output-placeholder">Agent 运行中，等待输出...</span>
      </div>
    );
  }

  return (
    <div className="agent-output">
      {lines.map(line => (
        <div key={line.id} className="agent-output-line">{line.text}</div>
      ))}
      <div ref={bottomRef} />
    </div>
  );
}
