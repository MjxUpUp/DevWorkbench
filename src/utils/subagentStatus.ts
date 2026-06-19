import type { ChatStreamEvent, SubagentDispatch, SubagentStatus } from '../types';

/**
 * Parse a dispatch_subagent tool_result into its terminal status. Mirrors the
 * Rust `parse_subagent_status` EXACTLY — both sides must agree on the same
 * prefixes (deer-flow subagent_status_contract.json + this project's own
 * [子 agent 结论] / [子 agent 失败] stamps). Returns null for non-terminal
 * streaming fragments.
 *
 * `Task polling timed out` MUST be checked before `Task timed out` (more
 * specific prefix), matching the Rust side.
 */
export function parseSubagentStatus(content: string): SubagentStatus | null {
  const t = content.trim();
  if (t.startsWith('[子 agent 结论]')) return 'completed';
  if (t.startsWith('[子 agent 失败')) return 'failed';
  if (t.startsWith('Task Succeeded')) return 'completed';
  if (t.startsWith('Task polling timed out')) return 'polling_timed_out';
  if (t.startsWith('Task timed out')) return 'timed_out';
  if (t.startsWith('Task cancelled')) return 'cancelled';
  if (t.startsWith('Task failed')) return 'failed';
  return null;
}

/**
 * Extract dispatch_subagent calls from an agent:event stream so the subagent
 * board can show concurrent fan-out + per-dispatch status. A tool_use starts a
 * 'running' dispatch; the next tool_result resolves the OLDEST running one
 * (the run loop emits Started×N then results×N in call order, so FIFO pairing
 * matches tool_use↔tool_result by position). Returns [] when the stream has no
 * dispatch_subagent calls (the common case — the board stays hidden).
 */
export function extractDispatches(
  events: ChatStreamEvent[] | null | undefined,
): SubagentDispatch[] {
  const dispatches: SubagentDispatch[] = [];
  for (const ev of events ?? []) {
    if (ev.kind === 'tool_use' && ev.name === 'dispatch_subagent') {
      const input = ev.input as { task?: string } | null;
      dispatches.push({ task: input?.task ?? '(无任务)', status: 'running' });
    } else if (ev.kind === 'tool_result') {
      const pending = dispatches.find((d) => d.status === 'running');
      if (pending) {
        const parsed = parseSubagentStatus(ev.content);
        pending.status = parsed ?? (ev.is_error ? 'failed' : 'completed');
      }
    }
  }
  return dispatches;
}
