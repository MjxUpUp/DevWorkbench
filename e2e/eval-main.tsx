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
(window as unknown as { __MOCK_INVOKE__: Record<string, unknown> }).__MOCK_INVOKE__ = {
  list_eval_cases: () => cases,
  list_verdicts: () => verdicts,
  eval_trend: () => trend,
};

const rootEl = document.getElementById('root');
if (rootEl) {
  createRoot(rootEl).render(
    <React.StrictMode>
      <EvalPanel />
    </React.StrictMode>,
  );
}
