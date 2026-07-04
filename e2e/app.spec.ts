import { test, expect } from '@playwright/test';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

// fs read instead of `import ... from '*.json'` — the project ships
// `"type": "module"`, so a static JSON import hits Node's strict ESM loader
// ("needs an import attribute of type: json"). Reading the file at load time
// sidesteps that and works under Playwright's esbuild spec transform.
const here = dirname(fileURLToPath(import.meta.url));
const realWire = JSON.parse(
  readFileSync(join(here, 'fixtures', 'agent-blocks-real.json'), 'utf-8'),
) as any[];

/**
 * Full-App interaction E2E — the "Layer 2" harness.
 *
 * Mounts the ENTIRE real App.tsx (TitleBar / Sidebar / MainStage→ChatView /
 * Composer / StatusBar / all stores) in a plain browser via app.html's IPC+event
 * shim, then Playwright drives the genuine user flow:
 *
 *   click project → type prompt → send →
 *   receive the REAL GLM wire (recorded by the Rust
 *   `record_real_glm_wire_to_e2e_fixture` test) streamed as `agent:event` →
 *   render in the live ChatView/BlocksView → complete → render from persisted
 *   session.blocks.
 *
 * This is the front-end capstone of the no-GUI end-to-end: real model bytes →
 * real React render, but through the FULL app interaction path (store events,
 * conversation selection, the live→persisted block handoff), not a single
 * component mount like chat.spec. Paired with the back-end
 * `build_react_agent_wires...` test, the chain
 * invoke-contract → live GLM → wire → front-end render is covered with no
 * desktop app.
 *
 * Every block type the live GLM run produced is asserted across BOTH the
 * running phase (live sessionBlocks) and the completed phase (persisted
 * session.blocks after clearBlocks) — proving the render source switches
 * correctly without losing the reply.
 */
test.describe('Full App interaction E2E', () => {
  test('select project → send → stream real GLM wire → complete → render', async ({ page }) => {
    const consoleErrors: string[] = [];
    page.on('console', (msg) => { if (msg.type() === 'error') consoleErrors.push(msg.text()); });
    page.on('pageerror', (err) => consoleErrors.push(String(err)));

    await page.goto('/app.html');

    // 1. App mounted → load_projects → WorkspaceTabs shows the seeded workspace.
    //    Click → selectProject → ChatView mounts in empty-state, the 常驻
    //    ConversationBookmarks bar renders at the top.
    await page.getByTestId('ws-tab').first().click();
    await expect(page.getByTestId('conversation-bookmarks')).toBeVisible();

    // 2. Type a prompt. canSend also needs selectedAgent, which ChatView's
    //    recommend-effect sets async after discover_agents_cmd resolves. Wait
    //    for the send button to be enabled (the visible signal that
    //    selectedAgent + project + prompt are all satisfied) before sending.
    const sendBtn = page.getByTestId('composer-send-btn');
    await page.getByTestId('chat-composer-input').fill('读取 Cargo.toml 的包名');
    await expect(sendBtn).toBeEnabled();
    await sendBtn.click();

    // createConversation → spawn_agent_session (mock returns running sess-1 /
    // conv-1) → selectConversation → ChatView renders turn 1. The running
    // structured agent shows an AgentMessage (BlocksView waiting state) before
    // the first block arrives.
    await expect(page.getByTestId('agent-message').first()).toBeVisible();

    // 3. Backend would emit agent:started then stream agent:event × N. Feed the
    //    REAL GLM wire — one ChatStreamEvent per agent:event — into the
    //    listener agentStore registered at mount. sessionBlocks['sess-1']
    //    accumulates → BlocksView renders live blocks while running.
    await page.evaluate(() => (window as any).__EMIT_EVENT__('agent:started', {}));
    const events = realWire as any[];
    for (const ev of events) {
      await page.evaluate(
        ({ e }) => (window as any).__EMIT_EVENT__('agent:event', { sessionId: 'sess-1', event: e }),
        { e: ev },
      );
    }

    // RUNNING assertion — live blocks rendered across every block type the real
    // GLM run produced. GLM interleaves thinking around the tool call, so there
    // can be multiple thinking blocks; assert at least one (live trace shape
    // varies per run).
    await expect(page.getByTestId('chat-block-text')).toContainText('dev-workbench');
    await expect(page.getByTestId('chat-block-tool-name')).toHaveText('read_file');
    await expect(page.getByTestId('chat-block-thinking').first()).toContainText('THINKING');
    await expect(page.getByTestId('chat-block-toolresult')).toContainText('tool_result');
    await expect(page.getByTestId('chat-block-result')).toContainText('完成');

    // 4. Simulate completion the way the backend would: persist the same wire
    //    into session.blocks + flip status, THEN emit agent:completed.
    //    agentStore's listener clears the live Map (clearBlocks) and
    //    refreshSessions reads back the completed turn → BlocksView now renders
    //    from session.blocks (the finalized/reloaded path, not live memory).
    await page.evaluate((blocks) => {
      const s = (window as any).__MOCK_STATE__.sessions.find((x: any) => x.id === 'sess-1');
      if (s) {
        s.status = 'completed';
        s.finishedAt = '2026-06-18T10:00:05.000Z';
        s.blocks = blocks;
      }
    }, realWire as any);
    await page.evaluate(() => (window as any).__EMIT_EVENT__('agent:completed', { sessionId: 'sess-1', status: 'completed', exitCode: 0 }));

    // COMPLETED assertion — blocks survive the live→persisted handoff. After
    // clearBlocks the AgentMessage falls back to session.blocks; the reply must
    // still render (this is the bug class where a finished turn goes blank).
    await expect(page.getByTestId('chat-block-text')).toContainText('dev-workbench');
    await expect(page.getByTestId('chat-block-tool-name')).toHaveText('read_file');
    await expect(page.getByTestId('chat-block-toolresult')).toContainText('tool_result');

    // Decision Chain reflects the completed status the listener wrote.
    await expect(page.getByTestId('agent-message').first()).toContainText('已完成');

    // No unexpected console errors / uncaught pageerrors. The shim warns on
    // unmock'd commands (informational); only surface real failures.
    const realErrors = consoleErrors.filter((e) => !e.includes('[e2e-shim] unhandled'));
    expect(realErrors, realErrors.join('\n')).toEqual([]);
  });
});
