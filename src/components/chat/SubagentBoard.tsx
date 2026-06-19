import type { ChatStreamEvent } from '../../types';
import { extractDispatches } from '../../utils/subagentStatus';

/**
 * C2/D3 subagent concurrency board. Renders ONLY when the current event stream
 * contains dispatch_subagent calls — shows how many children are in flight and
 * the per-dispatch status, so a wide fan-out is visible at a glance instead of
 * buried in the chat log. Pure: derives everything from the `events` prop, so
 * it stays trivially testable without wiring up the event subscription.
 */
export function SubagentBoard({
  events,
}: {
  events: ChatStreamEvent[] | null | undefined;
}) {
  const dispatches = extractDispatches(events);
  if (dispatches.length === 0) return null;
  const running = dispatches.filter((d) => d.status === 'running').length;
  const done = dispatches.length - running;
  return (
    <section className="subagent-board" aria-label="子 agent 并发看板">
      <header className="subagent-board-header">
        <span className="subagent-board-running" aria-label="运行中子 agent 数">
          {running} 运行中
        </span>
        <span className="subagent-board-done">{done} 已完成</span>
      </header>
      <ul className="subagent-board-list">
        {dispatches.map((d, i) => (
          <li
            key={i}
            className={`subagent-board-item subagent-board-item--${d.status}`}
          >
            <span className="subagent-board-status" data-status={d.status}>
              {d.status}
            </span>
            <span className="subagent-board-task">{d.task}</span>
          </li>
        ))}
      </ul>
    </section>
  );
}
