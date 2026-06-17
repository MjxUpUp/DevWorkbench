import React from 'react';
import { createRoot } from 'react-dom/client';
import { ModeSelector, type AgentMode } from '../src/components/ModeSelector';

// Mounts the real production ModeSelector and records every selection on
// window.__MODE_CHANGE__ so Playwright can assert which mode the user picked —
// the component under test is the genuine src code, only the harness wrapper
// is synthetic.
function Harness() {
  const [mode, setMode] = React.useState<AgentMode>('default');
  return (
    <ModeSelector
      value={mode}
      onChange={(m) => {
        (window as any).__MODE_CHANGE__ = m;
        setMode(m);
      }}
    />
  );
}

const rootEl = document.getElementById('root');
if (rootEl) {
  createRoot(rootEl).render(
    <React.StrictMode>
      <Harness />
    </React.StrictMode>,
  );
}
