import { test, expect } from '@playwright/test';

/**
 * Frontend E2E for the orchestrate canvas + node inspector.
 *
 * WHY THIS EXISTS — OrchestrateView.test.tsx mocks @xyflow/react into a
 * data-testid stub, so the unit tests structurally CANNOT see:
 *   - the real WorkflowNodeView rendering wf-node--status-<state> classes (#5),
 *   - the NodeInspector actually mounting when a node is clicked (the inspector
 *     unlock — OrchestrateView used to pass onSelectedChange, suppressing the
 *     builder's internal inspector),
 *   - the 状态 tab rendering BlocksView for accumulated node_output blocks (#8).
 * This harness runs the REAL OrchestrateView + REAL @xyflow/react canvas in a
 * real browser; only the Tauri IPC boundary is mocked (orchestrate.html).
 *
 * The spec drives the live store via window.__EMIT_EVENT__('workflow:progress',
 * { runId, event }) — the exact channel the backend emits on and OrchestrateView
 * listens to.
 */

const PROMPT_NODE = '.wf-node:has(.wf-node-id:has-text("prompt_1"))';

async function emit(page: import('@playwright/test').Page, event: Record<string, unknown>) {
  await page.evaluate((ev) => {
    (window as any).__EMIT_EVENT__('workflow:progress', { runId: 'run-e2e', event: ev });
  }, event);
}

test('clicking a canvas node opens the editable inspector (inspector unlock)', async ({ page }) => {
  await page.goto('/orchestrate.html');

  // Real ReactFlow rendered the prompt_1 node (not the jsdom stub).
  const node = page.locator(PROMPT_NODE);
  await expect(node).toBeVisible();

  // Before selecting: the inspector shows the empty hint, no field editors.
  await expect(page.getByText('点选一个节点编辑参数')).toBeVisible();

  // Click the node → onNodeClick → internal selection → NodeInspector mounts.
  await node.click();
  await expect(page.locator('.wf-inspector')).toBeVisible();
  await expect(page.getByText('文本')).toBeVisible();

  // The text field is an editable textarea (the unlock: it was hidden entirely
  // when onSelectedChange suppressed the internal inspector). Typing updates it.
  const textarea = page.locator('.wf-inspector textarea').first();
  await expect(textarea).not.toHaveAttribute('readonly', '');
  await textarea.fill('E2E typed prompt');
  await expect(textarea).toHaveValue('E2E typed prompt');
});

test('workflow:progress events color the canvas node (#5 runtime status)', async ({ page }) => {
  await page.goto('/orchestrate.html');
  const node = page.locator(PROMPT_NODE);
  await expect(node).toBeVisible();

  // No status class before any event (design-time type color shows instead).
  await expect(node).not.toHaveClass(/wf-node--status-/);

  // Emit the same event stream the backend emits: start → end(done).
  await emit(page, { kind: 'node_start', node: 'prompt_1' });
  await expect(node).toHaveClass(/wf-node--status-running/);

  await emit(page, { kind: 'node_end', node: 'prompt_1', status: 'done' });
  await expect(node).toHaveClass(/wf-node--status-done/);

  // A failure on another node colors it red.
  await emit(page, { kind: 'node_end', node: 'agent_1', status: 'failed', error: 'boom' });
  const agentNode = page.locator('.wf-node:has(.wf-node-id:has-text("agent_1"))');
  await expect(agentNode).toHaveClass(/wf-node--status-failed/);
});

test('状态 tab renders BlocksView for accumulated node_output blocks (#8)', async ({ page }) => {
  await page.goto('/orchestrate.html');
  await expect(page.locator(PROMPT_NODE)).toBeVisible();

  // Feed a real ChatStreamEvent chunk (the kernel maps AgentEvent→ChatStreamEvent
  // in executor.rs). applyEvent accumulates it into nodes[prompt_1].blocks.
  await emit(page, {
    kind: 'node_output',
    node: 'prompt_1',
    chunk: { kind: 'text', content: 'E2E blocks hello' },
  });

  // Switch to the 状态 (runtime) sidebar tab — it renders BlocksView per node.
  await page.getByRole('button', { name: '状态', exact: true }).click();

  // The accumulated text block renders inside the runtime tab's BlocksView
  // (.dag-node-output). Scope to it so the event-log line — which also echoes
  // the node_output preview "▸ prompt_1: E2E blocks hello" — can't satisfy
  // the assertion on its own.
  await expect(page.locator('.dag-node-output').getByText('E2E blocks hello')).toBeVisible();
});
