import React from 'react';
import { createRoot } from 'react-dom/client';
import { TriggerMenu } from '../src/components/TriggerMenu';

// Mounts the REAL production TriggerMenu with type="$" so Playwright drives the
// actual async list_skills path. Only the IPC boundary is mocked (via
// __TAURI_INTERNALS__ in trigger.html). Stub callbacks are fine — this harness
// only asserts the menu renders real installed-skill names.

function Harness() {
  // type from ?type= so the same harness covers $ (skills) and / (slash
  // commands); defaults to $ for the existing skill tests.
  const type = (new URLSearchParams(window.location.search).get('type') as '$' | '/' | '@') || '$';
  return (
    <TriggerMenu
      type={type}
      position={{ top: 0, left: 0 }}
      onSelect={() => {}}
      onClose={() => {}}
    />
  );
}

const rootEl = document.getElementById('root');
if (rootEl) {
  createRoot(rootEl).render(<Harness />);
}
