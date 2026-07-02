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
    scoreRubric: vi.fn(),
    runSession: vi.fn(),
    listRuns: vi.fn(),
    runPlatformMechanism: vi.fn(),
    runPlatformE2e: vi.fn(),
    runEnablement: vi.fn(),
  },
}));

import { evalApi } from '../../utils/evalApi';
import { useAgentStore } from '../../stores/agentStore';
import { useNavigationStore } from '../../stores/navigationStore';
import { EvalPanel, scoreOf } from '../dashboard/EvalPanel';
import type { EvalCaseRow, VerdictRow } from '../../utils/evalApi';

// A finished session the P3/A1 wizards can pick from.
function finishedSession(over: Partial<{ id: string; status: string; prompt: string; projectPath: string }> = {}) {
  return {
    id: 'sess-finished-1',
    status: 'completed',
    prompt: '修复 BlocksView 切分',
    projectPath: '/repo',
    ...over,
  } as never;
}

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
  // previewTrajectory returns the rich FullTrajectory shape now (steps + files
  // + tokens + cost + span tree) — P3 renders the summary line off it.
  vi.mocked(evalApi.previewTrajectory).mockResolvedValue({
    steps: [{ name: 'Read', status: null }, { name: 'Edit', status: null }],
    files_changed: ['src/a.ts'],
    input_tokens: 100,
    output_tokens: 20,
    cost_cents: 0.0072,
    span_tree: { roots: [{ kind: 'llm', name: 'glm-4.6', children: [{ kind: 'tool', name: 'Read' }] }] },
  });
  // P6 scoreRubric returns a clean 8-dim rubric (Q≈1, no hard gate).
  vi.mocked(evalApi.scoreRubric).mockResolvedValue({
    dims: [
      { key: 'tool_choice', label: '工具选择准确率', score: 1, val: '1.00' },
      { key: 'manual_intervention', label: 'manual intervention ⚠硬门', score: 1, val: '无', hard: true },
    ],
    q_code: 1,
    hard_gate_triggered: false,
  });
  // P4 platform-mechanism: the linear sample graph runs in order + reaches
  // done — a clean PASS verdict.
  vi.mocked(evalApi.runPlatformMechanism).mockResolvedValue({
    pass: true,
    actual_order: ['prompt_1', 'agent_1', 'gate_1'],
    actual_terminal: 'done',
    expected_order: ['prompt_1', 'agent_1', 'gate_1'],
    expected_terminal: 'done',
    mismatches: [],
  });
  // P4 平台-e2e: clean data-plane verdict — all set expectations hit (1 approved
  // case, 2 total, 1 eval-gate verdict, replay grade optimal).
  vi.mocked(evalApi.runPlatformE2e).mockResolvedValue({
    pass: true,
    checks: [
      { name: 'approved_case_count', pass: true, detail: '1' },
      { name: 'replay', pass: true, detail: 'optimal' },
    ],
    mismatches: [],
  });
  // P4 平台-加持: skills OFF→ON closed the expected gap (CLEAR improvement).
  vi.mocked(evalApi.runEnablement).mockResolvedValue({
    feature: 'skills',
    outcome: 'improvement',
    attribution: 'CLEAR',
    off_score: 0.4,
    on_score: 0.9,
    reason: 'ON 闭合了到 expected 的缺口',
  });
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

  it('P3 renders the rich trajectory summary (steps/files/tokens/cost) from previewTrajectory', async () => {
    useAgentStore.setState({ sessions: [finishedSession()] });
    mockAll({ cases: [case_()] });
    render(<EvalPanel />);
    fireEvent.click(screen.getByTestId('eval-nav-P3'));
    // Click 提取轨迹 → previewTrajectory fires → the rich summary line renders.
    fireEvent.click(screen.getByRole('button', { name: '提取轨迹' }));
    await waitFor(() => {
      expect(evalApi.previewTrajectory).toHaveBeenCalledWith('sess-finished-1');
    });
    // The mock returns 2 steps, 1 file, 100+20 tokens. The summary surfaces all.
    await waitFor(() => {
      expect(screen.getByText(/2 步轨迹/)).toBeInTheDocument();
    });
    expect(screen.getByText(/1 文件/)).toBeInTheDocument();
    expect(screen.getByText(/100\+20 tokens/)).toBeInTheDocument();
    expect(screen.getByText(/src\/a\.ts/)).toBeInTheDocument();
  });

  it('P6 renders the 8-dim rubric + Q_code from scoreRubric (locks the wiring)', async () => {
    // The latest eval verdict carries session_id + case_id → RubricCard can
    // assemble the rubric. A prior version showed a 3-row fake rubric with a
    // "needs scoring.rs extension" gap-note; the backend now computes 8 dims.
    mockAll({
      cases: [case_()],
      verdicts: [
        verdict({
          id: 'v-rubric',
          session_id: 'sess-replay-1',
          case_id: 'c1',
          gate: 'eval',
          verdict: 'PASS',
          attribution: 'CLEAR',
        }),
      ],
    });
    render(<EvalPanel />);
    fireEvent.click(screen.getByTestId('eval-nav-P6'));
    await waitFor(() => {
      // matcher defaults to exact_match inside the evalApi wrapper — the
      // component only passes session + case.
      expect(evalApi.scoreRubric).toHaveBeenCalledWith('sess-replay-1', 'c1');
    });
    // Q_code headline + the manual-intervention hard-gate row render.
    await waitFor(() => {
      expect(screen.getByText(/Q_code/)).toBeInTheDocument();
    });
    expect(screen.getByText('manual intervention ⚠硬门')).toBeInTheDocument();
  });

  it('A1 renders the span forest (LLM parent + tool child) from previewTrajectory', async () => {
    useAgentStore.setState({ sessions: [finishedSession()] });
    mockAll({ cases: [case_()] });
    render(<EvalPanel />);
    fireEvent.click(screen.getByTestId('eval-nav-A1'));
    // A1 auto-loads on session select; the mock's span tree has 1 LLM root
    // (glm-4.6) with a Read tool child. Both render — replaces the old empty
    // "A1 未接入" shell with a real span-tree view.
    await waitFor(() => {
      expect(screen.getByText(/1 个 LLM 父 span/)).toBeInTheDocument();
    });
    expect(screen.getByText('glm-4.6')).toBeInTheDocument();
    expect(screen.getByText('Read')).toBeInTheDocument();
  });

  it('P2 blocks save on malformed expected_steps_json (contract must be well-formed)', async () => {
    const c = case_();
    mockAll({ cases: [c] });
    render(<EvalPanel />);
    await waitFor(() => expect(screen.getByText('修复 BlocksView tool_use')).toBeInTheDocument());
    fireEvent.click(screen.getByText('修复 BlocksView tool_use'));
    await waitFor(() =>
      expect(screen.getByTestId('eval-feature-title')).toHaveTextContent('Case 详情 / 编辑'),
    );
    // Corrupt the expected-steps contract (non-JSON) → save must block, not
    // persist a malformed contract that score_eval_rubric would silently
    // mis-score. updateCase must NOT fire.
    const stepsBox = screen.getByDisplayValue('[{"name":"Read"}]') as HTMLTextAreaElement;
    fireEvent.change(stepsBox, { target: { value: 'not-json{' } });
    fireEvent.click(screen.getByText('保存契约'));
    await waitFor(() => {
      expect(screen.getByText(/校验失败：预期步骤须为 JSON 数组/)).toBeInTheDocument();
    });
    expect(evalApi.updateCase).not.toHaveBeenCalled();
  });

  it('F1 surfaces an honest insufficient-data state for a single trend point', async () => {
    mockAll({
      cases: [case_()],
      trend: [{ date: '2026-07-02', avg_score: 0.8, count: 1 } as never],
    });
    render(<EvalPanel />);
    await waitFor(() => expect(evalApi.trend).toHaveBeenCalled());
    fireEvent.click(screen.getByTestId('eval-nav-F1'));
    // A single point can't draw a regression line — the panel says so instead
    // of rendering a fake one-point "trend". (waitFor: trend loads async; at
    // click time it may still be [], which would show the 0-data branch.)
    await waitFor(() => {
      expect(screen.getByText(/仅 1 天数据/)).toBeInTheDocument();
    });
  });

  it('P4 runs the platform-mechanism eval (no LLM) and renders the verdict', async () => {
    // Selecting 平台-机制 used to show a gap-note ("需平台评测驱动，未接入").
    // Now it renders a real runner wired to eval_platform_mechanism — the only
    // platform object closed end-to-end; e2e/enablement stay gap-noted.
    mockAll({ cases: [case_()] });
    render(<EvalPanel />);
    fireEvent.click(screen.getByTestId('eval-nav-P4'));
    fireEvent.click(screen.getByText('平台-机制'));
    fireEvent.click(screen.getByRole('button', { name: /运行机制评测/ }));
    await waitFor(() => {
      expect(evalApi.runPlatformMechanism).toHaveBeenCalledWith(
        expect.stringContaining('prompt_1'),
        { seed: 'mechanism-eval' },
        { expect_order: ['prompt_1', 'agent_1', 'gate_1'], expect_terminal: 'done' },
      );
    });
    await waitFor(() => {
      expect(screen.getByText('PASS')).toBeInTheDocument();
    });
    expect(screen.getByText(/终态 done/)).toBeInTheDocument();
  });

  it('P4 runs the platform-e2e eval (data plane, no LLM) and renders the checks', async () => {
    // 平台-e2e was a gap-note; now a real runner drives the in-memory DB +
    // real logic functions and shows per-check pass/fail.
    mockAll({ cases: [case_()] });
    render(<EvalPanel />);
    fireEvent.click(screen.getByTestId('eval-nav-P4'));
    fireEvent.click(screen.getByText('平台-e2e'));
    // The default seed/expect textareas are pre-filled — just run.
    fireEvent.click(screen.getByRole('button', { name: /运行 e2e 评测/ }));
    await waitFor(() => {
      expect(evalApi.runPlatformE2e).toHaveBeenCalledWith(
        expect.objectContaining({ cases: expect.any(Array) }),
        expect.objectContaining({ approved_case_count: 1, total_case_count: 2 }),
      );
    });
    // The verdict is a clean PASS with the per-check list rendered.
    await waitFor(() => {
      expect(screen.getByText('PASS')).toBeInTheDocument();
    });
    expect(screen.getByText(/项检查/)).toBeInTheDocument();
    // The check rows render with a ✓ marker (scoped via getAllByText so the
    // pre-filled expect textarea, which also names approved_case_count, doesn't
    // collide with the rendered check).
    expect(screen.getAllByText(/approved_case_count/).length).toBeGreaterThanOrEqual(1);
  });

  it('P4 runs the platform-enablement eval (skills OFF→ON paired, CLEAR improvement)', async () => {
    // 平台-加持 was a gap-note; now a real runner fires runEnablement. Needs a
    // working dir (from sessions[0].projectPath) + a ready case to enable the
    // button (it stays disabled when workingDir/caseId are empty).
    useAgentStore.setState({ sessions: [finishedSession()] });
    mockAll({ cases: [case_()] });
    render(<EvalPanel />);
    // Wait for cases to load on the default P1 view before switching — the
    // enablement runner's case picker (readyCases) is empty until listCases
    // resolves, leaving the button disabled.
    await screen.findByText('修复 BlocksView tool_use');
    fireEvent.click(screen.getByTestId('eval-nav-P4'));
    fireEvent.click(screen.getByText('平台-加持'));
    fireEvent.click(screen.getByRole('button', { name: /运行加持评测/ }));
    await waitFor(() => {
      expect(evalApi.runEnablement).toHaveBeenCalledWith('c1', '/repo', 'exact_match');
    });
    // The verdict surfaces CLEAR + improvement + the off→on score delta.
    await waitFor(() => {
      expect(screen.getByText('CLEAR')).toBeInTheDocument();
    });
    expect(screen.getByText(/improvement/)).toBeInTheDocument();
    expect(screen.getByText(/0\.40.*0\.90/)).toBeInTheDocument();
  });

  it('scoreOf reads a numeric verdict string as its value (aligns with VerdictBadge)', () => {
    // VerdictBadge greens a leading-digit verdict ("0.85"); scoreOf used to
    // return 0 for the same row (verdict !== 'PASS'), so PairedCompare would
    // mis-score it and silently flip a CLEAR/BRAKE. Report score still wins.
    expect(scoreOf(verdict({ verdict: '0.85', report: null }))).toBeCloseTo(0.85);
    // Report's numeric score takes priority over the verdict string.
    expect(
      scoreOf(verdict({ verdict: '0.85', report: '{"score":0.9}' })),
    ).toBeCloseTo(0.9);
    // Non-numeric, non-PASS → 0 (FAIL/BRAKE).
    expect(scoreOf(verdict({ verdict: 'FAIL', report: null }))).toBe(0);
    // PASS with no report → 1.
    expect(scoreOf(verdict({ verdict: 'PASS', report: null }))).toBe(1);
  });
});
