import React from 'react';
import { createRoot } from 'react-dom/client';
import { TraceView } from '../src/components/trace/TraceView';
import { useNavigationStore } from '../src/stores/navigationStore';
import realTraces from './fixtures/llm-traces.json';

// Mounts TraceView against the recorded-shape rows DbTraceSink writes to
// llm_traces — a failed 400 turn (with its error response body) and a clean 200.
// The harness serves these from __MOCK_INVOKE__['list_llm_traces'], so TraceView's
// fetchTraces → invoke path runs against the real IPC boundary shape (cmd name +
// camelCase args). trace.spec drives a real browser to verify the timeline
// renders both rows and that a failed turn's response body is one click away —
// the diagnostic payoff of the whole feature (a 0.8s "stream failed: 400" turn
// becomes explainable without guessing).

// Scope TraceView to a session BEFORE render so its fetch effect fires on mount.
useNavigationStore.getState().setTrace('sess-41f2ddca');

// Serve the fixture through the IPC shim — real invoke(cmd, args) contract, no
// provider contacted, no credentials involved (the fixture carries only model /
// status / latency / token / example-error fields).
(window as any).__MOCK_INVOKE__ = {
  list_llm_traces: () => realTraces,
};

const rootEl = document.getElementById('root');
if (rootEl) {
  createRoot(rootEl).render(
    <React.StrictMode>
      <TraceView />
    </React.StrictMode>,
  );
}
