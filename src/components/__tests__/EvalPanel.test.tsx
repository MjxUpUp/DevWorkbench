import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent, within } from '@testing-library/react';

// chart.js <Line> touches canvas APIs jsdom lacks — stub it.
vi.mock('react-chartjs-2', () => ({ Line: () => <div data-testid="line-chart" /> }));
// Stub the whole evalApi surface so the panel never shells out to Tauri.
vi.mock('../../utils/evalApi', () => ({
  evalApi: {
    listCases: vi.fn(),
    listVerdicts: vi.fn(),
    trend: vi.fn(),
    getCase: vi.fn(),
    approveCase: vi.fn(),
    createCase: vi.fn(),
    updateCase: vi.fn(),
    runReplay: vi.fn(),
    previewTrajectory: vi.fn(),
    runSession: vi.fn(),
    listRuns: vi.fn(),
  },
}));

import { evalApi } from '../../utils/evalApi';
import { useAgentStore } from '../../stores/agentStore';
import { useNavigationStore } from '../../stores/navigationStore';
import { EvalPanel } from '../dashboard/EvalPanel';
import type { EvalCaseRow, VerdictRow } from '../../utils/evalApi';

function case_(over: Partial<EvalCaseRow> = {}): EvalCaseRow {
  return {
    id: 'c1',
    name: '修复 BlocksView tool_use',
    category: 'agent',
    input_prompt: 'edit 工具一直调用中',
    expected_steps_json: '[{"name":"Read"}]',
    expected_output: null,
    expected_observables_json: null,
    negative_json: null,
    source_session_id: 'a3f2c468',
    commit_sha: '2c16939',
    draft: 0,
    created_at: '2026-07-02T00:00:00Z',
    ...over,
  } as EvalCaseRow;
}

function verdict(over: Partial<VerdictRow> = {}): VerdictRow {
  return {
    id: 'v1',
    session_id: null,
    case_id: 'c1',
    gate: 'eval',
    verdict: 'PASS',
    attribution: 'CLEAR',
    report: '{"score":0.9,"actual_steps":["Read","Edit"],"negative_violated":false,"reason":"ok"}',
    commit_sha: null,
    created_at: '2026-07-02T14:02:00Z',
    ...over,
  } as VerdictRow;
}

function mockAll(p: { cases?: EvalCaseRow[]; verdicts?: VerdictRow[]; trend?: unknown[] }) {
  vi.mocked(evalApi.listCases).mockResolvedValue(p.cases ?? []);
  vi.mocked(evalApi.listVerdicts).mockResolvedValue(p.verdicts ?? []);
  vi.mocked(evalApi.trend).mockResolvedValue((p.trend ?? []) as never);
  vi.mocked(evalApi.getCase).mockResolvedValue(null);
  vi.mocked(evalApi.updateCase).mockResolvedValue(1);
  vi.mocked(evalApi.approveCase).mockResolvedValue(1);
  vi.mocked(evalApi.createCase).mockResolvedValue('new-id');
  vi.mocked(evalApi.previewTrajectory).mockResolvedValue([]);
}

describe('EvalPanel', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    useAgentStore.setState({ sessions: [] });
    useNavigationStore.setState({ activeProject: null });
  });

  it('P1 default: lists cases with includeDrafts and shows the pool', async () => {
    mockAll({ cases: [case_(), case_({ id: 'c2', name: 'compaction 跨模型', draft: 1 })] });
    render(<EvalPanel />);
    await waitFor(() => {
      expect(evalApi.listCases).toHaveBeenCalledWith({ includeDrafts: true, limit: 200 });
    });
    await waitFor(() => {
      expect(screen.getByText('修复 BlocksView tool_use')).toBeInTheDocument();
    });
    expect(screen.getByText('compaction 跨模型')).toBeInTheDocument();
  });

  it('navigates to V1 and shows the verdicts ledger filtered by gate', async () => {
    mockAll({
      cases: [case_()],
      verdicts: [
        verdict(),
        verdict({ id: 'v2', gate: 'honesty', verdict: 'FAIL', attribution: 'BRAKE' }),
      ],
    });
    render(<EvalPanel />);
    await waitFor(() => expect(evalApi.listVerdicts).toHaveBeenCalled());
    // Switch to V1.
    fireEvent.click(screen.getByTestId('eval-nav-V1'));
    expect(screen.getByTestId('eval-feature-title')).toHaveTextContent('Verdicts 查询');
    // The honesty row rendered (scoped to the table so the filter <option>'s
    // gate name doesn't collide) — proves the ledger shows both verdicts.
    const table = screen.getByTestId('eval-verdict-table');
    expect(within(table).getByText('honesty')).toBeInTheDocument();
    expect(within(table).getByText('eval')).toBeInTheDocument();
  });

  it('V1 filter narrows to a single gate', async () => {
    mockAll({
      cases: [case_()],
      verdicts: [
        verdict({ id: 'v1', gate: 'eval' }),
        verdict({ id: 'v2', gate: 'honesty', verdict: 'FAIL', attribution: null }),
      ],
    });
    render(<EvalPanel />);
    await waitFor(() => expect(evalApi.listVerdicts).toHaveBeenCalled());
    fireEvent.click(screen.getByTestId('eval-nav-V1'));
    const table = screen.getByTestId('eval-verdict-table');
    // Both gates visible initially (scoped to the table so the <option> texts
    // in the filter <select> don't collide).
    expect(within(table).getByText('eval')).toBeInTheDocument();
    expect(within(table).getByText('honesty')).toBeInTheDocument();
    // Filter to honesty only — the eval row must disappear from the table.
    fireEvent.change(screen.getByDisplayValue('全部 gate'), { target: { value: 'honesty' } });
    expect(within(table).queryByText('eval')).not.toBeInTheDocument();
    expect(within(table).getByText('honesty')).toBeInTheDocument();
  });

  it('F2 flywheel surfaces real counts + broken-chain state when no failures', async () => {
    mockAll({
      cases: [case_(), case_({ id: 'c2' })],
      verdicts: [verdict()], // a PASS, no FAIL → failCount 0 → broken chain
    });
    render(<EvalPanel />);
    await waitFor(() => expect(evalApi.listVerdicts).toHaveBeenCalled());
    fireEvent.click(screen.getByTestId('eval-nav-F2'));
    expect(screen.getByText('断链 · 失败未沉淀（V2 空）')).toBeInTheDocument();
  });

  it('P5 paired compare flags net-improve when the new run beats the old (regression guard)', async () => {
    // list_verdicts is new-first: [0]=本次(0.9 PASS), [1]=上次(0.5 FAIL).
    // A prior swap of oldV/newV mislabeled this as 回归 — the brake/admit
    // verdict inverted. This locks the fix: a real improvement must readmit.
    mockAll({
      cases: [case_()],
      verdicts: [
        verdict({
          id: 'v-new',
          case_id: 'c1',
          gate: 'eval',
          verdict: 'PASS',
          attribution: 'CLEAR',
          report: '{"score":0.9,"actual_steps":["Read","Edit"],"negative_violated":false}',
          created_at: '2026-07-02T14:02:00Z',
        }),
        verdict({
          id: 'v-old',
          case_id: 'c1',
          gate: 'eval',
          verdict: 'FAIL',
          attribution: 'BRAKE',
          report: '{"score":0.5,"actual_steps":["Read","Bash"],"negative_violated":true}',
          created_at: '2026-07-01T14:02:00Z',
        }),
      ],
    });
    render(<EvalPanel />);
    await waitFor(() => expect(evalApi.listVerdicts).toHaveBeenCalled());
    fireEvent.click(screen.getByTestId('eval-nav-P5'));
    expect(screen.getByText('净提升 · 可准入')).toBeInTheDocument();
    expect(screen.queryByText('回归 · 拦')).not.toBeInTheDocument();
  });

  it('P2 detail saves the edited contract via updateCase (input_prompt stays locked)', async () => {
    const c = case_();
    mockAll({ cases: [c] });
    render(<EvalPanel />);
    await waitFor(() => expect(screen.getByText('修复 BlocksView tool_use')).toBeInTheDocument());
    // Open P2 by clicking the case row (P1 onSelect → P2).
    fireEvent.click(screen.getByText('修复 BlocksView tool_use'));
    await waitFor(() => {
      expect(screen.getByTestId('eval-feature-title')).toHaveTextContent('Case 详情 / 编辑');
    });
    // The input_prompt renders locked (read-only div), not as an input.
    expect(screen.getByText('edit 工具一直调用中')).toBeInTheDocument();
    // Edit name + save.
    const nameInput = screen.getByDisplayValue('修复 BlocksView tool_use');
    fireEvent.change(nameInput, { target: { value: 'renamed case' } });
    fireEvent.click(screen.getByText('保存契约'));
    await waitFor(() => {
      expect(evalApi.updateCase).toHaveBeenCalledWith(
        'c1',
        expect.objectContaining({
          name: 'renamed case',
          // input_prompt is NOT in the update payload (C1 locked).
          expectedStepsJson: '[{"name":"Read"}]',
        }),
      );
    });
    expect(screen.getByText('已保存契约字段')).toBeInTheDocument();
  });

  it('surfaces a load failure', async () => {
    vi.mocked(evalApi.listCases).mockRejectedValue(new Error('db locked'));
    vi.mocked(evalApi.listVerdicts).mockResolvedValue([]);
    vi.mocked(evalApi.trend).mockResolvedValue([]);
    render(<EvalPanel />);
    await waitFor(() => {
      expect(screen.getByText(/加载失败/)).toBeInTheDocument();
    });
  });
});
