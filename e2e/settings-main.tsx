import React from 'react';
import { createRoot } from 'react-dom/client';
import { ProvidersSection } from '../src/components/settings/ProvidersSection';
import { MemorySection } from '../src/components/settings/MemorySection';
import { SkillsSection } from '../src/components/settings/SkillsSection';
import { HooksSection } from '../src/components/settings/HooksSection';
import { PluginsSection } from '../src/components/settings/PluginsSection';
import { ToastProvider } from '../src/components/Toast';
import { useNavigationStore } from '../src/stores/navigationStore';

// Mounts the REAL production settings sections (the genuine src components) so
// Playwright drives the actual React render + browser events. Only the IPC
// boundary is mocked (via __TAURI_INTERNALS__ in settings.html). Three sections
// are mounted together; each test seeds the commands its target section needs.

// memory/skills scope on the active project — seed it BEFORE mount so their
// loadForProject / loadCatalog calls resolve against a known project.
useNavigationStore.setState({
  activeProject: { path: '/proj/e2e', name: 'e2e-proj' } as never,
});

function Harness() {
  return (
    <ToastProvider>
      <h2>providers</h2>
      <section data-e2e="providers">
        <ProvidersSection />
      </section>
      <h2>memory</h2>
      <section data-e2e="memory">
        <MemorySection />
      </section>
      <h2>skills</h2>
      <section data-e2e="skills">
        <SkillsSection />
      </section>
      <h2>hooks</h2>
      <section data-e2e="hooks">
        <HooksSection />
      </section>
      <h2>capability</h2>
      <section data-e2e="capability">
        <PluginsSection />
      </section>
    </ToastProvider>
  );
}

const rootEl = document.getElementById('root');
if (rootEl) {
  createRoot(rootEl).render(<Harness />);
}
