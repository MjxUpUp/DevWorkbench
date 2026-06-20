import type { ChatStreamEvent } from '../../types';
import { extractDispatches } from '../../utils/subagentStatus';

/**
 * C2/D3 subagent concurrency board. Renders ONLY when the current event stream
 * contains dispatch_subagent calls — shows how many children are in flight and
 * the per-dispatch status, so a wide fan-out is visible at a glance instead of
 * buried in the chat log. C2: each resolved dispatch also shows its per-child
 * token + cost attribution, and the header rolls up the fan-out's total cost —
 * the anti-"10× cost" visibility that makes multi-agent spend legible (prereq
 * B3/B5 cost infra now in place). Pure: derives everything from the `events`
 * prop, so it stays trivially testable without wiring up the event subscription.
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
  // C2 aggregate cost: only dispatches that resolved with a cost footer
  // contribute. Running ones (no footer yet) are excluded — showing a partial
  // total would understate a still-running fan-out.
  const costRows = dispatches.filter((d) => d.costUsd != null);
  const totalCost = costRows.reduce((s, d) => s + (d.costUsd ?? 0), 0);
  const totalIn = costRows.reduce((s, d) => s + (d.inputTokens ?? 0), 0);
  const totalOut = costRows.reduce((s, d) => s + (d.outputTokens ?? 0), 0);
  return (
    <section className="subagent-board" aria-label="子 agent 并发看板">
      <header className="subagent-board-header">
        <span className="subagent-board-running" aria-label="运行中子 agent 数">
          {running} 运行中
        </span>
        <span className="subagent-board-done">{done} 已完成</span>
        {costRows.length > 0 && (
          <span
            className="subagent-board-cost"
            aria-label="子 agent 合计成本"
            title={`本组 fan-out 合计: ${totalIn} 输入 / ${totalOut} 输出 token`}
          >
            合计 {totalIn}→{totalOut} tok · ${totalCost.toFixed(4)}
          </span>
        )}
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
            {d.costUsd != null && (
              <span
                className="subagent-board-item-cost"
                title="该子 agent 的 token + 成本（C2 per-dispatch 归因）"
              >
                {d.inputTokens}→{d.outputTokens} tok · ${d.costUsd.toFixed(4)}
              </span>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
