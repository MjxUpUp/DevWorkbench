import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';

// chart.js <Line> touches canvas APIs jsdom lacks — stub it to a plain div.
vi.mock('react-chartjs-2', () => ({ Line: () => <div data-testid="line-chart" /> }));
// Stub evalApi so the panel never shells out to Tauri in unit tests.
vi.mock('../../utils/evalApi', () => ({
  evalApi: {
    trend: vi.fn(),
    listRuns: vi.fn(),
  },
}));

import { evalApi } from '../../utils/evalApi';
import { EvalPanel } from '../dashboard/EvalPanel';

describe('EvalPanel', () => {
  beforeEach(() => {
    vi.mocked(evalApi.trend).mockReset();
    vi.mocked(evalApi.listRuns).mockReset();
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
});
