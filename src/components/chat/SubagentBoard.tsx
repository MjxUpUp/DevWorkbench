import { useState } from 'react';
import type { ChatStreamEvent, SubagentStatus } from '../../types';
import { extractDispatches } from '../../utils/subagentStatus';

type ViewMode = 'list' | 'kanban' | 'fleet';

/**
 * C2/D3 subagent concurrency board. Renders ONLY when the current event stream
 * contains dispatch_subagent calls — shows how many children are in flight and
 * the per-dispatch status, so a wide fan-out is visible at a glance instead of
 * buried in the chat log. C2: each resolved dispatch also shows its per-child
 * token + cost attribution, and the header rolls up the fan-out's total cost —
 * the anti-"10× cost" visibility that makes multi-agent spend legible (prereq
 * B3/B5 cost infra now in place). Pure: derives everything from the `events`
 * prop, so it stays trivially testable without wiring up the event subscription.
 *
 * Three view modes (mirrors Linear/Cursor's subagent UX):
 *  - list:   compact one-line-per-dispatch (default; good for ≤5 children)
 *  - kanban: columns by status (running / done / failed); good for wide fan-out
 *  - fleet:  card grid with full task text + cost; good for cost audit
 */
const VIEW_LABELS: Record<ViewMode, string> = {
  list: '列表',
  kanban: '看板',
  fleet: '卡片',
};

const KANBAN_COLUMNS: { key: ('running' | SubagentStatus)[]; label: string; cls: string }[] = [
  { key: ['running'], label: '运行中', cls: 'running' },
  { key: ['completed'], label: '已完成', cls: 'done' },
  { key: ['failed', 'timed_out', 'polling_timed_out', 'cancelled'], label: '失败/超时', cls: 'failed' },
];

export function SubagentBoard({
  events,
}: {
  events: ChatStreamEvent[] | null | undefined;
}) {
  const dispatches = extractDispatches(events);
  const [view, setView] = useState<ViewMode>('list');
  if (dispatches.length === 0) return null;
  const running = dispatches.filter((d) => d.status === 'running').length;
  const done = dispatches.length - running;
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
        <div className="subagent-board-views" role="tablist" aria-label="视图切换">
          {(Object.keys(VIEW_LABELS) as ViewMode[]).map((v) => (
            <button
              key={v}
              type="button"
              role="tab"
              aria-selected={view === v}
              className={`subagent-board-view-btn${view === v ? ' active' : ''}`}
              onClick={() => setView(v)}
              title={VIEW_LABELS[v]}
            >
              {VIEW_LABELS[v]}
            </button>
          ))}
        </div>
      </header>

      {view === 'list' && (
        <ul className="subagent-board-list">
          {dispatches.map((d, i) => (
            <li key={i} className={`subagent-board-item subagent-board-item--${d.status}`}>
              <span className="subagent-board-status" data-status={d.status}>{d.status}</span>
              <span className="subagent-board-task">{d.task}</span>
              {d.costUsd != null && (
                <span className="subagent-board-item-cost" title="该子 agent 的 token + 成本（C2 per-dispatch 归因）">
                  {d.inputTokens}→{d.outputTokens} tok · ${d.costUsd.toFixed(4)}
                </span>
              )}
            </li>
          ))}
        </ul>
      )}

      {view === 'kanban' && (
        <div className="subagent-board-kanban">
          {KANBAN_COLUMNS.map((col) => {
            const items = dispatches.filter((d) => col.key.includes(d.status));
            if (items.length === 0) return null;
            return (
              <div key={col.cls} className={`subagent-board-col subagent-board-col--${col.cls}`}>
                <div className="subagent-board-col-header">{col.label} ({items.length})</div>
                <div className="subagent-board-col-body">
                  {items.map((d, i) => (
                    <div key={i} className={`subagent-board-card subagent-board-card--${d.status}`}>
                      <span className="subagent-board-status" data-status={d.status}>{d.status}</span>
                      <span className="subagent-board-task">{d.task}</span>
                      {d.costUsd != null && (
                        <span className="subagent-board-item-cost">
                          ${d.costUsd.toFixed(4)}
                        </span>
                      )}
                    </div>
                  ))}
                </div>
              </div>
            );
          })}
        </div>
      )}

      {view === 'fleet' && (
        <div className="subagent-board-fleet">
          {dispatches.map((d, i) => (
            <div key={i} className={`subagent-board-fleet-card subagent-board-card--${d.status}`}>
              <div className="subagent-board-fleet-head">
                <span className="subagent-board-status" data-status={d.status}>{d.status}</span>
                {d.costUsd != null && (
                  <span className="subagent-board-item-cost">
                    {d.inputTokens}→{d.outputTokens} tok · ${d.costUsd.toFixed(4)}
                  </span>
                )}
              </div>
              <div className="subagent-board-fleet-task">{d.task}</div>
            </div>
          ))}
        </div>
      )}
    </section>
  );
}
