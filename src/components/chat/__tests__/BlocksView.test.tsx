import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { BlocksView } from '../BlocksView';
import type { ChatStreamEvent } from '../../../types';

// run_workflow_graph tool_use mounts WorkflowProgressStrip, which subscribes
// to a Tauri event. Stub listen so the strip's effect is inert under jsdom.
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(() => Promise.resolve(() => {})),
}));

// compact-card expand calls invoke('read_compact_archive_cmd'). Default stub
// returns null (no archive file on disk); per-test overrides via mockResolvedValue.
const invokeMock = vi.fn(
  (_cmd: string, _args?: unknown): Promise<unknown> => Promise.resolve(null),
);
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (cmd: string, args?: unknown) => invokeMock(cmd, args),
}));

/**
 * BlocksView 测试 — v3 重构后选择器从 .chat-block-* class 改为 data-testid
 * （class 已迁移到 CSS module，加 hash 后缀）。断言意图不变。
 *
 * v3 行为变化（已反映在断言）：
 * - thinking 卡用 L1Thinking 组件，label 是 "THINKING"/"THOUGHT FOR Ns"，
 *   不再有「思考过程」中文字样
 * - tool_use/tool_result 用 L2ToolPill，desc 字段从 input 提炼
 * - tool_result 不再有「工具错误」字样，desc 是 content 截断
 */
describe('BlocksView', () => {
  it('renders each block kind in order, one card per event', () => {
    const events: ChatStreamEvent[] = [
      { kind: 'text', content: 'hello world' },
      { kind: 'tool_use', name: 'Read', input: { file_path: '/a.txt' } },
      { kind: 'tool_result', content: 'file contents here', is_error: false },
      { kind: 'result', is_error: false, secs: 12 },
    ];
    const { container } = render(<BlocksView events={events} running={false} />);

    expect(screen.getByText('hello world')).toBeInTheDocument();
    expect(screen.getByText('Read')).toBeInTheDocument();
    // result 卡的「完成」标签 + 耗时秒数
    const resultCard = screen.getByTestId('chat-block-result');
    expect(resultCard).toHaveTextContent('完成');
    expect(resultCard).toHaveTextContent('12s');
    // 4 个顶层 block 卡（text/tool_use/tool_result/result），各带 data-testid。
    // 精确到顶层卡 testid：内嵌钩子（chat-block-tool-name / -toolresult-head）
    // 也以 chat-block- 开头，前缀计数会把它们也算进来抬高到 6，故按具体顶层
    // testid 断言。验证意图不变——4 种 block 各渲染一张卡。
    const topCards = container.querySelectorAll(
      '[data-testid="chat-block-text"], [data-testid="chat-block-tool"], [data-testid="chat-block-toolresult"], [data-testid="chat-block-result"]',
    );
    expect(topCards.length).toBe(4);
    // 非运行无流式光标
    expect(screen.queryByTestId('chat-streaming-cursor')).toBeNull();
  });

  it('shows a streaming caret while running', () => {
    render(<BlocksView events={[{ kind: 'text', content: 'x' }]} running={true} />);
    expect(screen.getByTestId('chat-streaming-cursor')).toBeInTheDocument();
  });

  it('marks a failed result block with the failed class', () => {
    render(<BlocksView events={[{ kind: 'result', is_error: true, secs: 3 }]} running={false} />);
    expect(screen.getByText('失败')).toBeInTheDocument();
    const result = screen.getByTestId('chat-block-result');
    // failed 态有对应的 class（CSS module hash）
    expect(result.className).toMatch(/failed/);
  });

  it('expands tool_use input on click (collapsed by default)', () => {
    render(<BlocksView events={[{ kind: 'tool_use', name: 'Bash', input: { command: 'ls -la' } }]} running={false} />);

    // 折叠态：input JSON 不在文档
    expect(screen.queryByText(/"command": "ls -la"/)).not.toBeInTheDocument();

    fireEvent.click(screen.getByRole('button'));
    expect(screen.getByText(/"command": "ls -la"/)).toBeInTheDocument();
  });

  it('renders tool_use string input verbatim when expanded', () => {
    render(<BlocksView events={[{ kind: 'tool_use', name: 'Grep', input: 'pattern' }]} running={false} />);
    fireEvent.click(screen.getByRole('button'));
    expect(screen.getByText('pattern')).toBeInTheDocument();
  });

  it('renders a thinking block as a collapsible card, collapsed by default', () => {
    const { container } = render(
      <BlocksView events={[{ kind: 'thinking', content: 'let me reason about this' }]} running={false} />,
    );
    // v3：thinking 用 L1Thinking，data-testid=chat-block-thinking
    expect(container.querySelector('[data-testid="chat-block-thinking"]')).not.toBeNull();
    // label 是 THINKING（无 secs 时）
    expect(screen.getByText('THINKING')).toBeInTheDocument();
    // 折叠态：trace 内容不在文档
    expect(screen.queryByText('let me reason about this')).not.toBeInTheDocument();
  });

  it('expands the thinking trace on click', () => {
    render(<BlocksView events={[{ kind: 'thinking', content: 'step by step plan' }]} running={false} />);
    fireEvent.click(screen.getByRole('button'));
    expect(screen.getByText('step by step plan')).toBeInTheDocument();
  });

  it('merges consecutive per-delta thinking events into a single card', () => {
    const { container } = render(
      <BlocksView
        events={[
          { kind: 'thinking', content: 'step 1. ' },
          { kind: 'thinking', content: 'step 2. ' },
          { kind: 'thinking', content: 'step 3.' },
        ]}
        running={false}
      />,
    );
    // 合并成 1 个 thinking 卡
    expect(container.querySelectorAll('[data-testid="chat-block-thinking"]').length).toBe(1);
    fireEvent.click(screen.getByRole('button'));
    expect(screen.getByText('step 1. step 2. step 3.')).toBeInTheDocument();
  });

  it('does NOT merge thinking across a different-kind block in between', () => {
    const { container } = render(
      <BlocksView
        events={[
          { kind: 'thinking', content: 'first' },
          { kind: 'text', content: 'answer' },
          { kind: 'thinking', content: 'second' },
        ]}
        running={false}
      />,
    );
    expect(container.querySelectorAll('[data-testid="chat-block-thinking"]').length).toBe(2);
  });

  it('merges consecutive text events the same way', () => {
    const { container } = render(
      <BlocksView
        events={[
          { kind: 'text', content: 'foo ' },
          { kind: 'text', content: 'bar ' },
          { kind: 'text', content: 'baz' },
        ]}
        running={false}
      />,
    );
    expect(container.querySelectorAll('[data-testid="chat-block-text"]').length).toBe(1);
    expect(screen.getByText('foo bar baz')).toBeInTheDocument();
  });

  it('marks an errored tool result', () => {
    render(<BlocksView events={[{ kind: 'tool_result', content: 'boom', is_error: true }]} running={false} />);
    // v3：errored tool_result 用 L2ToolPill status=error，wrap 带 errWrap class
    const card = screen.getByTestId('chat-block-toolresult');
    expect(card.className).toMatch(/err/i);
  });

  it('renders a waiting hint + caret for an empty running stream', () => {
    const { container } = render(<BlocksView events={[]} running={true} />);
    expect(container.querySelectorAll('[data-testid^="chat-block-"]').length).toBe(0);
    expect(screen.getByText('等待模型响应')).toBeInTheDocument();
    expect(screen.getByTestId('chat-streaming-cursor')).toBeInTheDocument();
  });

  it('renders nothing for an empty completed stream', () => {
    render(<BlocksView events={[]} running={false} />);
    expect(screen.queryByTestId('chat-streaming-cursor')).toBeNull();
  });

  it('renders a file_changed block as a path line', () => {
    render(<BlocksView events={[{ kind: 'file_changed', path: '/src/app.rs' }]} running={false} />);
    expect(screen.getByText('/src/app.rs')).toBeInTheDocument();
    expect(screen.getByTestId('chat-block-file')).not.toBeNull();
  });

  it('derives a friendly node-count desc for run_workflow_graph', () => {
    render(
      <BlocksView
        events={[
          {
            kind: 'tool_use',
            name: 'run_workflow_graph',
            input: { graph: { nodes: { a: {}, b: {}, c: {} }, edges: [], start: 'a', end: 'c' } },
          },
        ]}
        running={true}
      />,
    );
    expect(screen.getByText(/自规划工作流 · 3 节点/)).toBeInTheDocument();
  });

  it('renders a compact summary card with dropped-count badge (collapsed, no sessionId)', () => {
    render(
      <BlocksView
        events={[
          { kind: 'compact', summary: '已压缩历史', archived_at: null, dropped_count: 12, is_error: false },
        ]}
        running={false}
      />,
    );
    const card = screen.getByTestId('chat-block-compact');
    expect(card).toHaveTextContent('已压缩历史');
    expect(card).toHaveTextContent('-12 msg');
    // 无 sessionId：展开按钮禁用，不触发 invoke
    expect(screen.queryByTestId('chat-block-compact-archive')).toBeNull();
  });

  it('renders a compact error card with the error class', () => {
    render(
      <BlocksView
        events={[
          { kind: 'compact', summary: '压缩熔断', archived_at: null, dropped_count: 0, is_error: true },
        ]}
        running={false}
      />,
    );
    const card = screen.getByTestId('chat-block-compact');
    expect(card.className).toMatch(/isError/);
    // 熔断态不可展开（canExpand = !is_error）
    expect(card.querySelector('button')).toHaveAttribute('disabled');
  });

  it('expands the compact card and loads the archive via invoke', async () => {
    invokeMock.mockResolvedValueOnce([
      {
        ts: '2026-07-02T00:00:00Z',
        kind: 'summarize',
        summary: '早期工具调用摘要',
        dropped_count: 2,
        dropped_messages: [
          { role: 'user', content: 'do something' },
          { role: 'assistant', content: 'tool result blob' },
        ],
      },
    ]);
    render(
      <BlocksView
        events={[
          { kind: 'compact', summary: '已压缩历史', archived_at: '/tmp/x.jsonl', dropped_count: 2, is_error: false },
        ]}
        running={false}
        sessionId="sess-compact-1"
      />,
    );
    fireEvent.click(screen.getByTestId('chat-block-compact').querySelector('button')!);
    await waitFor(() => {
      expect(screen.getByTestId('chat-block-compact-archive')).toBeInTheDocument();
    });
    expect(invokeMock).toHaveBeenCalledWith('read_compact_archive_cmd', { sessionId: 'sess-compact-1' });
    expect(screen.getByTestId('chat-block-compact-archive')).toHaveTextContent('早期工具调用摘要');
    expect(screen.getByTestId('chat-block-compact-archive')).toHaveTextContent('do something');
  });
});
