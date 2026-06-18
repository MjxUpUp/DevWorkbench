import React from 'react';
import { createRoot } from 'react-dom/client';
import { BlocksView } from '../src/components/chat/BlocksView';
import realWire from './fixtures/agent-blocks-real.json';

// Mounts BlocksView with a REAL GLM wire — recorded from a live GLM run by the
// Rust `record_real_glm_wire_to_e2e_fixture` test (src-tauri), not a hand-written
// mock. The Playwright chat.spec drives a real browser against this, verifying
// the front-end deserializes + renders the genuine agent:event payload the back
// end emits, across every block type the model actually produced.
const rootEl = document.getElementById('root');
if (rootEl) {
  createRoot(rootEl).render(
    <React.StrictMode>
      <BlocksView events={realWire as any} running={false} />
    </React.StrictMode>,
  );
}
