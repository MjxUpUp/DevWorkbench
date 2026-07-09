import React from 'react';
import { createRoot } from 'react-dom/client';
import { EvalPanel } from '../src/components/dashboard/EvalPanel';
import { useNavigationStore } from '../src/stores/navigationStore';
import { useAgentStore } from '../src/stores/agentStore';
// Pull in the app's full stylesheet so the harness page isn't unstyled when a
// human opens /eval.html (E2E asserts only on text/testids, so the missing
// import went unnoticed — same root cause that once affected the e2e harness).
import '../src/styles/index.css';
import cases from './fixtures/eval-cases.json';
import verdicts from './fixtures/eval-verdicts.json';
import trend from './fixtures/eval-trend.json';

// Mounts EvalPanel against the recorded-shape rows the back-end eval modules
// return — two cases (ready + probe), three verdicts across eval/honesty gates
// (with CLEAR + BRAKE attribution), and a 2-point regression trend. The harness
// serves these from __MOCK_INVOKE__['list_eval_cases' / 'list_verdicts' /
// 'eval_trend'], so EvalPanel's useEvalData → invoke path runs against the real
// IPC boundary shape (cmd name + camelCase args). eval.spec drives a real
// browser to verify the three anti-gaming principles are VISIBLE end-to-end:
// 客观事实 (locked prompt in P2) / 因果归因 (CLEAR+BRAKE badges in V1) /
// 配对回放 (P5 net-improve when the new run beats the old).

// P4 platform-enablement needs a working dir (activeProject.path) for its
// button to be live — the runner stays disabled until caseId + workingDir are
// both set. The real driver runs two live agents (needs a provider key); here
// run_eval_enablement is served from the IPC shim so the spec can click 运行加持评测
// and assert the verdict renders without a key. The other replay (agent obj)
// still needs a live LLM and is not driven here.
useNavigationStore.setState({ activeProject: { path: '/repo' } } as never);
useAgentStore.setState({ sessions: [] } as never);

// Serve the fixtures through the IPC shim — real invoke(cmd, args) contract,
// no provider contacted, no credentials involved (the fixtures carry only
// gate / verdict / attribution / score / step-sequence fields).
//
// score_eval_rubric returns the 8-dim rubric the backend computes for the
// newest eval verdict's session×case (c-ready: expected [Read,Grep,Edit],
// negative [Bash]; the PASS run touched no forbidden tool → Q≈1, no hard gate).
// preview_session_trajectory returns a rich trajectory (P3 summary + A1 span
// tree) so those views render real shapes if the spec navigates to them.
(window as unknown as { __MOCK_INVOKE__: Record<string, unknown> }).__MOCK_INVOKE__ = {
  list_eval_cases: () => cases,
  list_verdicts: () => verdicts,
  eval_trend: () => trend,
  score_eval_rubric: () => ({
    dims: [
      { key: 'tool_choice', label: '工具选择准确率', score: 1, val: '1.00' },
      { key: 'attr_hallucination', label: 'attribute hallucination', score: 1, val: '无' },
      { key: 'correctness_loop', label: 'correctness-loop 迭代', score: 1, val: '0 次' },
      { key: 'manual_intervention', label: 'manual intervention ⚠硬门', score: 1, val: '无', hard: true },
      { key: 'dryrun_pass', label: 'dryrun pass', score: 1, val: '3/3' },
      { key: 'harness_pattern', label: 'harness-pattern', score: 1, val: '1.00' },
      { key: 'dsl', label: 'DSL 声明符合', score: 1, val: '1.00' },
      { key: 'file_change', label: '文件变更符合预期', score: 1, val: '1.00' },
    ],
    q_code: 1,
    hard_gate_triggered: false,
  }),
  preview_session_trajectory: () => ({
    steps: [
      { name: 'Read', status: null },
      { name: 'Grep', status: null },
      { name: 'Edit', status: null },
    ],
    files_changed: ['src/BlocksView.tsx'],
    input_tokens: 1200,
    output_tokens: 180,
    cost_cents: 0.083,
    span_tree: {
      roots: [
        {
          kind: 'llm',
          name: 'glm-4.6',
          latency_ms: 420,
          children: [
            { kind: 'tool', name: 'Read' },
            { kind: 'tool', name: 'Grep' },
            { kind: 'tool', name: 'Edit' },
          ],
        },
      ],
    },
  }),
  // P4 platform-e2e: the default seed (1 approved + 1 draft case, 1 eval-gate
  // verdict) drives the in-memory DB + real logic functions to a clean PASS —
  // all set expectations hit. Lets the spec click 运行 e2e 评测 and assert the
  // per-check verdict renders.
  eval_platform_e2e: () => ({
    pass: true,
    checks: [
      { name: 'approved_case_count', pass: true, detail: '1' },
      { name: 'total_case_count', pass: true, detail: '2' },
      { name: 'verdict_count_for_gate', pass: true, detail: 'eval=1' },
      { name: 'replay', pass: true, detail: 'optimal' },
    ],
    mismatches: [],
  }),
  // P4 platform-enablement: skills OFF→ON closed the expected gap (CLEAR
  // improvement). The real driver runs two live agents (needs a provider key);
  // here it is served from the IPC shim so the spec can assert the runner wires
  // run_eval_enablement and renders the verdict without a key.
  run_eval_enablement: () => ({
    feature: 'skills',
    outcome: 'improvement',
    attribution: 'CLEAR',
    off_score: 0.4,
    on_score: 0.9,
    reason: 'ON 闭合了到 expected 的缺口',
  }),
  // SA 平台自审: 前端 invoke 集合 (F, 构建期 grep src/ 生成的 INVOKED_COMMANDS
  // manifest) vs 后端 generate_handler! 注册集合 (B, include_str! lib.rs) 对齐。
  // CoverageSelfAudit 展开传入 IPC — 真实后端在编译期嵌入 B；这里 mock 一个零死按钮
  // PASS 态（少量死代码 WARN），让 spec 验证 manifest 真接线 + 渲染结论。F\B 死按钮的
  // FAIL 形态由 EvalPanel 单测（coverage-dead-buttons）覆盖。
  eval_platform_coverage: () => ({
    pass: true,
    frontend_count: 107,
    backend_count: 109,
    aligned_count: 107,
    dead_buttons: [],
    dead_code: ['list_llm_traces', 'prune_llm_traces_now'],
    checks: [
      { name: 'frontend_invoke_count', pass: true, detail: '107 commands' },
      { name: 'aligned', pass: true, detail: '107 对齐' },
    ],
  }),
};

const rootEl = document.getElementById('root');
if (rootEl) {
  createRoot(rootEl).render(
    <React.StrictMode>
      <EvalPanel />
    </React.StrictMode>,
  );
}
