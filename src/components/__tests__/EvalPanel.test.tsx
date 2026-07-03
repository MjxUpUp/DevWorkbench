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
    runPlatformCoverage: vi.fn(),
  },
}));

import { evalApi } from '../../utils/evalApi';
import { useAgentStore } from '../../stores/agentStore';
import { useNavigationStore } from '../../stores/navigationStore';
import { EvalPanel, scoreOf, parseVerdictTrace } from '../dashboard/EvalPanel';
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
  // SA 平台自审 default: 零死按钮 PASS，少量死代码 WARN（事件/内部 command）。
  vi.mocked(evalApi.runPlatformCoverage).mockResolvedValue({
    pass: true,
    frontend_count: 107,
    backend_count: 110,
    aligned_count: 107,
    dead_buttons: [],
    dead_code: ['list_llm_traces', 'prune_llm_traces_now'],
    checks: [
      { name: 'frontend_invoke_count', pass: true, detail: '107 commands' },
      { name: 'aligned', pass: true, detail: '107 commands 对齐' },
    ],
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

  it('SA 平台自审: 零死按钮 → PASS + 死代码 WARN 区可见', async () => {
    mockAll({ cases: [case_()] });
    render(<EvalPanel />);
    fireEvent.click(screen.getByTestId('eval-nav-SA'));
    expect(screen.getByTestId('eval-feature-title')).toHaveTextContent('IPC 接线自审');
    await waitFor(() => {
      expect(screen.getByTestId('coverage-verdict')).toHaveTextContent('PASS');
    });
    // 真调了 IPC，传入了构建期 manifest 的前端 invoke 集合。
    expect(evalApi.runPlatformCoverage).toHaveBeenCalled();
    // 零死按钮 → 不渲染死按钮区。
    expect(screen.queryByTestId('coverage-dead-buttons')).toBeNull();
    // 死代码区（WARN）渲染并列出未调用的 command。
    expect(screen.getByTestId('coverage-dead-code')).toHaveTextContent('list_llm_traces');
  });

  it('SA 平台自审: 有死按钮 → FAIL + 逐个列出死按钮（前端调了后端没注册）', async () => {
    mockAll({ cases: [case_()] });
    vi.mocked(evalApi.runPlatformCoverage).mockResolvedValue({
      pass: false,
      frontend_count: 108,
      backend_count: 110,
      aligned_count: 107,
      dead_buttons: ['definitely_not_registered_xyz'],
      dead_code: [],
      checks: [
        {
          name: 'dead_button:definitely_not_registered_xyz',
          pass: false,
          detail: '前端 invoke 但后端 generate_handler! 未注册（死按钮）',
        },
      ],
    });
    render(<EvalPanel />);
    fireEvent.click(screen.getByTestId('eval-nav-SA'));
    await waitFor(() => {
      expect(screen.getByTestId('coverage-verdict')).toHaveTextContent('FAIL');
    });
    // 死按钮逐个列出——反刷分抓造假的核心信号。
    expect(screen.getByTestId('coverage-dead-buttons')).toHaveTextContent(
      'definitely_not_registered_xyz',
    );
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
    // __all__ 聚合视图默认：c1 净提升 → "净提升 · 可准入（1/1）"。正则匹配保留序
    // 守卫——若 [newV, oldV] 反号会聚合出 "回归 · 拦（N/N）"，被 /回归 · 拦/ 捉到。
    expect(screen.getByText(/净提升 · 可准入/)).toBeInTheDocument();
    expect(screen.queryByText(/回归 · 拦/)).not.toBeInTheDocument();
  });

  it('P5 __all__ aggregate surfaces 分歧 (split) when cases mix improve + regress', async () => {
    // 反刷分 mixed 场景：c1 提升 / c2 回归并存 → 不简单放/拦，标分歧待人审。
    // 回归守卫：split 分支曾被 regresses>0 三元首分支吞掉（死代码），mixed 永远
    // 显示"回归·拦"。修复后 split 优先，mixed 走分歧。
    mockAll({
      cases: [case_(), case_({ id: 'c2', name: 'case 2' })],
      verdicts: [
        // c1: new(0.9) > old(0.5) → improve
        verdict({ id: 'c1-new', case_id: 'c1', verdict: 'PASS', report: '{"score":0.9}', created_at: '2026-07-02T14:02:00Z' }),
        verdict({ id: 'c1-old', case_id: 'c1', verdict: 'FAIL', report: '{"score":0.5}', created_at: '2026-07-01T14:02:00Z' }),
        // c2: new(0.5) < old(0.9) → regress
        verdict({ id: 'c2-new', case_id: 'c2', verdict: 'FAIL', report: '{"score":0.5}', created_at: '2026-07-02T14:02:00Z' }),
        verdict({ id: 'c2-old', case_id: 'c2', verdict: 'PASS', report: '{"score":0.9}', created_at: '2026-07-01T14:02:00Z' }),
      ],
    });
    render(<EvalPanel />);
    await waitFor(() => expect(evalApi.listVerdicts).toHaveBeenCalled());
    fireEvent.click(screen.getByTestId('eval-nav-P5'));
    expect(screen.getByText(/分歧 · 待判/)).toBeInTheDocument();
    // split 优先于纯 regresses>0：mixed 不一刀切拦。
    expect(screen.queryByText(/回归 · 拦/)).not.toBeInTheDocument();
  });

  it('P5 __all__ aggregate surfaces 持平 when every case is same (no improve, no regress)', async () => {
    // 反刷分诚实信号：全 same 不该标"净提升"——曾落入净提升分支显示
    // "净提升·可准入（0/N）"，标签撒谎（无提升却说净提升）。应标"持平·待判"。
    mockAll({
      cases: [case_(), case_({ id: 'c2', name: 'case 2' })],
      verdicts: [
        // c1: new(0.9) == old(0.9) → same
        verdict({ id: 'c1-new', case_id: 'c1', verdict: 'PASS', report: '{"score":0.9}', created_at: '2026-07-02T14:02:00Z' }),
        verdict({ id: 'c1-old', case_id: 'c1', verdict: 'PASS', report: '{"score":0.9}', created_at: '2026-07-01T14:02:00Z' }),
        // c2: new(0.5) == old(0.5) → same
        verdict({ id: 'c2-new', case_id: 'c2', verdict: 'FAIL', report: '{"score":0.5}', created_at: '2026-07-02T14:02:00Z' }),
        verdict({ id: 'c2-old', case_id: 'c2', verdict: 'FAIL', report: '{"score":0.5}', created_at: '2026-07-01T14:02:00Z' }),
      ],
    });
    render(<EvalPanel />);
    await waitFor(() => expect(evalApi.listVerdicts).toHaveBeenCalled());
    fireEvent.click(screen.getByTestId('eval-nav-P5'));
    expect(screen.getByText(/持平 · 待判/)).toBeInTheDocument();
    // 全 same 不该标"净提升"。
    expect(screen.queryByText(/净提升 · 可准入/)).not.toBeInTheDocument();
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

  it('A1 renders the paired span forests (LLM parent + tool child) from previewTrajectory', async () => {
    // A1 是 case 驱动双栏：取 c1 的新旧 eval verdicts（带 session_id）→ 两次
    // previewTrajectory → 左右双栏 span 树。seed 两条 eval verdict，序 new-first。
    mockAll({
      cases: [case_()],
      verdicts: [
        verdict({
          id: 'v-new',
          case_id: 'c1',
          gate: 'eval',
          session_id: 'sess-new',
          verdict: 'PASS',
          attribution: 'CLEAR',
          report: '{"score":0.9}',
          created_at: '2026-07-02T14:02:00Z',
        }),
        verdict({
          id: 'v-old',
          case_id: 'c1',
          gate: 'eval',
          session_id: 'sess-old',
          verdict: 'FAIL',
          attribution: 'BRAKE',
          report: '{"score":0.5}',
          created_at: '2026-07-01T14:02:00Z',
        }),
      ],
    });
    render(<EvalPanel />);
    fireEvent.click(screen.getByTestId('eval-nav-A1'));
    // 双栏各加载一次 previewTrajectory（新 + 旧 session）。
    await waitFor(() => {
      expect(evalApi.previewTrajectory).toHaveBeenCalledWith('sess-new');
      expect(evalApi.previewTrajectory).toHaveBeenCalledWith('sess-old');
    });
    // mock span 树两边都含 glm-4.6 LLM 父 + Read tool 子（双栏 → 出现 ≥2 次）。
    await waitFor(() => {
      expect(screen.getAllByText('glm-4.6').length).toBeGreaterThanOrEqual(2);
    });
    expect(screen.getAllByText('Read').length).toBeGreaterThanOrEqual(2);
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

  it('parseVerdictTrace mirrors backend gates.rs parse_verdict (adversarial contract)', () => {
    // ① 首行 VERDICT: PASS 契约命中 + body 无 fail marker → PASS
    expect(parseVerdictTrace('VERDICT: PASS\nlooks good').passed).toBe(true);
    // ① 首行 PASS 但 body 有 fail marker → 覆盖为 FAIL（对抗性：冲突信号判 FAIL）
    expect(parseVerdictTrace('VERDICT: PASS\nhowever, one defect remains.').passed).toBe(false);
    // ① 首行 VERDICT: FAIL → FAIL
    expect(parseVerdictTrace('VERDICT: FAIL\nfound a bug').passed).toBe(false);
    // ③ 无契约 → keyword fallback：pass 词且无 fail marker → PASS
    expect(parseVerdictTrace('The implementation is correct and passes review.').passed).toBe(true);
    // ③ fail marker 主导 → FAIL
    expect(parseVerdictTrace('大致通过，但存在缺陷。').passed).toBe(false);
    // ③ 默认 FAIL（模糊/空 · 对抗性默认）
    expect(parseVerdictTrace('').passed).toBe(false);
    // 违约契约（首行仅含 VERDICT: PASS 但前置 defect）→ 降级 keyword → FAIL
    expect(parseVerdictTrace('The work has defects. VERDICT: PASS').passed).toBe(false);
  });

  it('P1 flags a stale case whose source session is gone (⚠ 来源缺失)', async () => {
    // case source_session_id='a3f2c468'，但 sessions 里没有它 → 来源失效。
    mockAll({ cases: [case_()] });
    render(<EvalPanel />);
    await screen.findByText('⚠ 来源缺失');
    expect(screen.queryByText(/#a3f2c468/)).not.toBeInTheDocument();
  });

  it('P1 clears stale when the source session is still live', async () => {
    useAgentStore.setState({ sessions: [{ id: 'a3f2c468', status: 'completed' } as never] });
    mockAll({ cases: [case_()] });
    render(<EvalPanel />);
    await screen.findByText(/#a3f2c468/);
    expect(screen.queryByText('⚠ 来源缺失')).not.toBeInTheDocument();
  });

  it('F1 surfaces the new vs old commit version-compare card with delta + admit', async () => {
    // 两个 commit 的 eval verdicts：新 commit 均分高 → 准入。verdicts 已带 commit_sha，
    // 前端按 commit 派生（eval_trend 后端无 version 维度）。
    mockAll({
      cases: [case_()],
      verdicts: [
        verdict({
          id: 'v-new',
          case_id: 'c1',
          gate: 'eval',
          verdict: 'PASS',
          commit_sha: 'new1111',
          report: '{"score":0.9}',
          created_at: '2026-07-02T14:02:00Z',
        }),
        verdict({
          id: 'v-old',
          case_id: 'c1',
          gate: 'eval',
          verdict: 'FAIL',
          commit_sha: 'old0000',
          report: '{"score":0.5}',
          created_at: '2026-07-01T14:02:00Z',
        }),
      ],
      trend: [
        { date: '2026-07-01', avg_score: 0.5, count: 1 },
        { date: '2026-07-02', avg_score: 0.9, count: 1 },
      ],
    });
    render(<EvalPanel />);
    fireEvent.click(screen.getByTestId('eval-nav-F1'));
    // 版本对比卡片渲染：新 0.9 / 旧 0.5 / delta +0.400 / 准入。
    await waitFor(() => {
      expect(screen.getByText(/新旧版本均分对比/)).toBeInTheDocument();
    });
    expect(screen.getByText(/净提升 · 可准入/)).toBeInTheDocument();
    expect(screen.getByText('+0.400')).toBeInTheDocument();
  });
});
