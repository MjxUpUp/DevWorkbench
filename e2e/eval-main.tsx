import React from 'react';
import { createRoot } from 'react-dom/client';
import { EvalPanel } from '../src/components/dashboard/EvalPanel';
import { useNavigationStore } from '../src/stores/navigationStore';
import { useAgentStore } from '../src/stores/agentStore';
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

// P4 falls back to activeProject.path for the working dir; null is fine — the
// E2E does not drive a replay (replay needs a live LLM provider). It only
// checks the 4 evaluation-object radios render + the honest gap-note.
useNavigationStore.setState({ activeProject: null } as never);
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
  // P4 platform-mechanism: the default linear sample graph (prompt → agent →
  // gate) runs in order + reaches done — a clean PASS. Lets the spec click
  // 运行机制评测 and assert the verdict renders (the only platform object
  // closed end-to-end; e2e/enablement stay gap-noted).
  eval_platform_mechanism: () => ({
    pass: true,
    actual_order: ['prompt_1', 'agent_1', 'gate_1'],
    actual_terminal: 'done',
    expected_order: ['prompt_1', 'agent_1', 'gate_1'],
    expected_terminal: 'done',
    mismatches: [],
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
