import React from 'react';
import { createRoot } from 'react-dom/client';
// OrchestrateView (like most views) does NOT import its own CSS — the real app
// pulls orchestrate.css in globally via styles/index.css. Mount it here too, or
// .wf-builder's grid + .wf-canvas height collapse (canvas height:0) and ReactFlow
// parks the nodes off-canvas where Playwright can't click them.
import '../src/styles/index.css';
import { OrchestrateView } from '../src/components/orchestrate/OrchestrateView';
import { useNavigationStore } from '../src/stores/navigationStore';

// Mounts the REAL OrchestrateView — including the REAL @xyflow/react canvas
// (OrchestrateView.test.tsx stubs ReactFlow to a data-testid div, so the live
// WorkflowNodeView rendering, status classes, and inspector are never exercised
// by unit tests). Only the Tauri IPC boundary is mocked (orchestrate.html).
// The spec drives the workflow:progress event stream via window.__EMIT_EVENT__.

// OrchestrateView reads activeProject from navigationStore (run button disabled
// without one). Seed it before mount so the view is usable on first paint.
useNavigationStore.setState({
  activeProject: {
    id: 'p1',
    name: 'Dev Workbench',
    description: '主项目',
    path: 'E:/DevWorkbench',
    tags: ['tauri', 'react'],
    cover_image: null,
    open_count: 1,
    last_opened_at: null,
    starred: true,
    created_at: '2025-01-01T00:00:00.000Z',
    last_opened_tools: [],
    workspace_tools: [],
  },
  activeView: 'orchestrate',
  sidebarOpen: true,
  selectedConversationId: null,
});

function Harness() {
  return <OrchestrateView />;
}

const rootEl = document.getElementById('root');
if (rootEl) {
  // No StrictMode: its mount→unmount→remount would register two workflow:
  // progress listeners and double-apply every emitted event (see app.html notes).
  createRoot(rootEl).render(<Harness />);
}
