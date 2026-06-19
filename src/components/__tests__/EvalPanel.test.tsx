import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';

// chart.js <Line> touches canvas APIs jsdom lacks — stub it to a plain div.
vi.mock('react-chartjs-2', () => ({ Line: () => <div data-testid="line-chart" /> }));
// Stub evalApi so the panel never shells out to Tauri in unit tests.
vi.mock('../../utils/evalApi', () => ({
  evalApi: {
    trend: vi.fn(),
    listRuns: vi.fn(),
    runSession: vi.fn(),
  },
}));

import { evalApi } from '../../utils/evalApi';
import { useAgentStore } from '../../stores/agentStore';
import { EvalPanel } from '../dashboard/EvalPanel';
import type { Session } from '../../types';

function makeSession(over: Partial<Session> = {}): Session {
  return {
    id: 's1',
    projectPath: '/p',
    agentType: 'claude_code',
    status: 'completed',
    prompt: 'do the thing',
    model: null,
    startedAt: '2026-06-19T00:00:00Z',
    finishedAt: '2026-06-19T00:01:00Z',
    exitCode: 0,
    outputSummary: null,
    contextSnapshot: null,
    linkedRequirementId: null,
    parentSessionId: null,
    conversationId: null,
    ...over,
  } as Session;
}

describe('EvalPanel', () => {
  beforeEach(() => {
    vi.mocked(evalApi.trend).mockReset();
    vi.mocked(evalApi.listRuns).mockReset();
    vi.mocked(evalApi.runSession).mockReset();
    useAgentStore.setState({ sessions: [] });
  });

  it('shows the empty hint when there is no trend data', async () => {
    vi.mocked(evalApi.trend).mockResolvedValue([]);
    vi.mocked(evalApi.listRuns).mockResolvedValue([]);
    render(<EvalPanel />);
    await waitFor(() => {
      expect(screen.getByText(/暂无评估数据/)).toBeInTheDocument();
    });
    expect(evalApi.trend).toHaveBeenCalledWith(30);
    expect(evalApi.listRuns).toHaveBeenCalledWith(undefined, 20);
  });

  it('renders the chart + run rows when data is present', async () => {
    vi.mocked(evalApi.trend).mockResolvedValue([
      { date: '2026-06-19', avg_score: 0.8, count: 2 },
    ]);
    vi.mocked(evalApi.listRuns).mockResolvedValue([
      {
        id: 'r1',
        session_id: 's1',
        conversation_id: null,
        matcher: 'exact_match',
        score: 1.0,
        grade: 'optimal',
        steps: 3,
        created_at: '2026-06-19T00:00:00Z',
      },
      {
        id: 'r2',
        session_id: 's2',
        conversation_id: null,
        matcher: 'any_order',
        score: 0.0,
        grade: 'incorrect',
        steps: 0,
        created_at: '2026-06-19T00:00:01Z',
      },
    ]);
    render(<EvalPanel />);
    await waitFor(() => {
      expect(screen.getByTestId('line-chart')).toBeInTheDocument();
    });
    expect(screen.getByText('最优')).toBeInTheDocument();
    expect(screen.getByText('错误')).toBeInTheDocument();
    expect(screen.getByText('1.00')).toBeInTheDocument();
    expect(screen.getByText('0.00')).toBeInTheDocument();
  });

  it('surfaces a load failure message', async () => {
    vi.mocked(evalApi.trend).mockRejectedValue(new Error('db locked'));
    vi.mocked(evalApi.listRuns).mockResolvedValue([]);
    render(<EvalPanel />);
    await waitFor(() => {
      expect(screen.getByText(/加载失败/)).toBeInTheDocument();
    });
  });

  it('disables the run button when no finished session exists', async () => {
    // A running session is mid-stream — not scoreable, so the dropdown is empty.
    useAgentStore.setState({ sessions: [makeSession({ id: 'r', status: 'running' })] });
    vi.mocked(evalApi.trend).mockResolvedValue([]);
    vi.mocked(evalApi.listRuns).mockResolvedValue([]);
    render(<EvalPanel />);
    await waitFor(() => {
      expect(screen.getByText('运行评估').closest('button')).toBeDisabled();
    });
    expect(screen.getByText('暂无已完成的会话')).toBeInTheDocument();
  });

  it('runs an evaluation with the parsed reference and shows the grade', async () => {
    useAgentStore.setState({
      sessions: [makeSession({ id: 'sx', status: 'completed', agentType: 'claude_code' })],
    });
    vi.mocked(evalApi.trend).mockResolvedValue([]);
    vi.mocked(evalApi.listRuns).mockResolvedValue([]);
    vi.mocked(evalApi.runSession).mockResolvedValue({
      id: 'rnew',
      session_id: 'sx',
      conversation_id: null,
      matcher: 'in_order',
      score: 0.66,
      grade: 'suboptimal',
      steps: 4,
      created_at: '2026-06-19T00:00:00Z',
    });
    render(<EvalPanel />);
    await waitFor(() => {
      expect(screen.getByText('运行评估').closest('button')).not.toBeDisabled();
    });
    // Pick in_order matcher + a multi-line golden reference.
    fireEvent.change(screen.getByDisplayValue(/精确匹配/), { target: { value: 'in_order' } });
    fireEvent.change(screen.getByPlaceholderText(/Read/), {
      target: { value: 'Read\nGrep, Bash\n\n' },
    });
    fireEvent.click(screen.getByText('运行评估'));
    await waitFor(() => {
      expect(screen.getByText(/本次：次优/)).toBeInTheDocument();
    });
    expect(evalApi.runSession).toHaveBeenCalledWith('sx', 'in_order', ['Read', 'Grep', 'Bash']);
  });

  it('omits reference when the textarea is blank (reference-free heuristic)', async () => {
    useAgentStore.setState({
      sessions: [makeSession({ id: 'sy', status: 'completed' })],
    });
    vi.mocked(evalApi.trend).mockResolvedValue([]);
    vi.mocked(evalApi.listRuns).mockResolvedValue([]);
    vi.mocked(evalApi.runSession).mockResolvedValue({
      id: 'r2',
      session_id: 'sy',
      conversation_id: null,
      matcher: 'exact_match',
      score: 1.0,
      grade: 'optimal',
      steps: 2,
      created_at: '2026-06-19T00:00:00Z',
    });
    render(<EvalPanel />);
    await waitFor(() => {
      expect(screen.getByText('运行评估')).not.toBeDisabled();
    });
    fireEvent.click(screen.getByText('运行评估'));
    await waitFor(() => {
      expect(evalApi.runSession).toHaveBeenCalledWith('sy', 'exact_match', undefined);
    });
  });
});
