import React from 'react';
import { createRoot } from 'react-dom/client';
// IMPORTANT: do NOT import src/main.tsx — it calls installDevMock(), whose
// transformCallback is a stub returning 0 (callbacks never fire). That breaks
// the event bus this harness depends on (agentStore's agent:event listener).
// app.html's inline shim owns __TAURI_INTERNALS__ directly with a real callback
// table + event bus.
import App from '../src/App';

const rootEl = document.getElementById('root');
if (rootEl) {
  createRoot(rootEl).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}
