import React from 'react';
import { createRoot } from 'react-dom/client';
import { FileChanges } from '../src/components/chat/FileChanges';
import type { Session } from '../src/types';

// A session whose contextSnapshot lists changed files. The checkpoint probe
// (get_checkpoint) is answered by the harness's mocked invoke (see index.html),
// driven per-test by Playwright via window.__MOCK_INVOKE__.
const session: Session = {
  id: 's1',
  projectPath: '/proj/e2e',
  agentType: 'claude_code',
  status: 'completed',
  prompt: '',
  model: null,
  startedAt: '2026-06-17T00:00:00Z',
  finishedAt: null,
  exitCode: 0,
  outputSummary: null,
  contextSnapshot: { filesChanged: ['a.rs', 'b.ts'], keyOutput: '' },
  linkedRequirementId: null,
  parentSessionId: null,
  conversationId: null,
};

const rootEl = document.getElementById('root');
if (rootEl) {
  createRoot(rootEl).render(
    <React.StrictMode>
      <FileChanges session={session} />
    </React.StrictMode>,
  );
}
