import { describe, it, expect } from 'vitest';
import { parseSubagentStatus, parseCostLine, extractDispatches } from '../subagentStatus';
import type { ChatStreamEvent } from '../../types';

describe('parseSubagentStatus', () => {
  it('parses this project dispatch_subagent prefixes', () => {
    expect(parseSubagentStatus('[子 agent 结论] 调研完成,3 页报告')).toBe('completed');
    expect(parseSubagentStatus('[子 agent 失败: model 400]')).toBe('failed');
  });

  it('parses deer-flow contract cases (both sides must agree)', () => {
    expect(parseSubagentStatus('Task Succeeded. Result: investigated')).toBe('completed');
    expect(parseSubagentStatus('Task failed. Error: RuntimeError')).toBe('failed');
    expect(parseSubagentStatus('Task cancelled by user.')).toBe('cancelled');
    expect(parseSubagentStatus('Task timed out. Error: 900 seconds')).toBe('timed_out');
    expect(
      parseSubagentStatus('Task polling timed out after 15 minutes. Status: RUNNING'),
    ).toBe('polling_timed_out');
  });

  it('checks polling-timed-out before timed-out (more specific prefix)', () => {
    // "Task polling timed out" must not be eaten by the "Task timed out" branch.
    expect(parseSubagentStatus('Task polling timed out after 1 minutes')).toBe(
      'polling_timed_out',
    );
  });

  it('returns null for non-terminal fragments and tolerates whitespace', () => {
    expect(parseSubagentStatus('Investigating ...')).toBeNull();
    expect(parseSubagentStatus('  Task Succeeded. Result: ok  ')).toBe('completed');
    expect(parseSubagentStatus('  Task cancelled by user.\n')).toBe('cancelled');
  });
});

describe('parseCostLine', () => {
  it('parses the C2 wire shape the Rust format_cost_line emits', () => {
    // Must mirror `📊 子 agent 用量: A→B tok · $C` exactly — a drift blanks the
    // board, so guard the literal arrow (→) and · separator here.
    const parsed = parseCostLine(
      '[子 agent 结论] 报告完成\n\n📊 子 agent 用量: 1234→567 tok · $0.0123',
    );
    expect(parsed).toEqual({ inputTokens: 1234, outputTokens: 567, costUsd: 0.0123 });
  });

  it('returns null when there is no cost footer', () => {
    // Running dispatch / test model / child made no tracked call → no footer.
    expect(parseCostLine('[子 agent 结论] done')).toBeNull();
    expect(parseCostLine('Task Succeeded. ok')).toBeNull();
    expect(parseCostLine('')).toBeNull();
  });
});

describe('extractDispatches', () => {
  const tu = (input: unknown): ChatStreamEvent => ({
    kind: 'tool_use',
    name: 'dispatch_subagent',
    input,
  });
  const tr = (content: string, is_error = false): ChatStreamEvent => ({
    kind: 'tool_result',
    content,
    is_error,
  });

  it('returns [] when the stream has no dispatch_subagent calls', () => {
    expect(extractDispatches(null)).toEqual([]);
    expect(extractDispatches(undefined)).toEqual([]);
    expect(extractDispatches([{ kind: 'text', content: 'hi' }])).toEqual([]);
  });

  it('marks a dispatched child running until a tool_result resolves it', () => {
    expect(extractDispatches([tu({ task: '研究 X' })])).toEqual([
      { task: '研究 X', status: 'running' },
    ]);
  });

  it('resolves status via parseSubagentStatus, pairing tool_result FIFO', () => {
    // Run loop emits Started×N then results×N in call order, so the first
    // tool_result pairs with the first dispatch, the second with the second.
    const evs: ChatStreamEvent[] = [
      tu({ task: '研究 X' }),
      tu({ task: '研究 Y' }),
      tr('[子 agent 结论] done-x'),
      tr('[子 agent 失败: 500]'),
    ];
    expect(extractDispatches(evs)).toEqual([
      { task: '研究 X', status: 'completed' },
      { task: '研究 Y', status: 'failed' },
    ]);
  });

  it('falls back to is_error when the content has no known prefix', () => {
    expect(extractDispatches([tu({ task: 'Z' }), tr('odd', true)])[0].status).toBe('failed');
    expect(extractDispatches([tu({ task: 'Z' }), tr('odd', false)])[0].status).toBe('completed');
  });

  it('uses a placeholder task when the tool_use input is missing', () => {
    expect(extractDispatches([tu(null)])[0].task).toBe('(无任务)');
  });

  it('attributes per-dispatch cost from a tool_result cost footer (C2)', () => {
    // The backend stamps `📊 子 agent 用量: …` on the resolved result; the board
    // reads inputTokens/outputTokens/costUsd off the SAME content it parses
    // status from. Absent footer → cost fields stay undefined.
    const withCost = extractDispatches([
      tu({ task: '研究 A' }),
      tr('[子 agent 结论] done-a\n\n📊 子 agent 用量: 1000→200 tok · $0.0044'),
    ]);
    expect(withCost[0]).toMatchObject({
      status: 'completed',
      inputTokens: 1000,
      outputTokens: 200,
      costUsd: 0.0044,
    });

    const noCost = extractDispatches([tu({ task: '研究 B' }), tr('[子 agent 结论] done-b')]);
    expect(noCost[0].costUsd).toBeUndefined();
    expect(noCost[0].inputTokens).toBeUndefined();
  });
});
