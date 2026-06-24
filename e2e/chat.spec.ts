import { test, expect } from '@playwright/test';

/**
 * Front-end rendering E2E driven by REAL GLM output. The wire events fed to
 * BlocksView were recorded from a live GLM run (Rust
 * `record_real_glm_wire_to_e2e_fixture` test in src-tauri) — not hand-written —
 * so this verifies the front-end deserializes and renders the genuine
 * agent:event payload the back end emits, across every block type the model
 * actually produced (thinking, tool_use, tool_result, text, result).
 *
 * This is the front-end half of the no-GUI end-to-end: real model bytes → real
 * React render. Paired with the back-end `build_react_agent_wires...` test, the
 * two together cover invoke-contract → live GLM → wire → front-end render
 * without ever spinning up the Tauri desktop app.
 */
test('BlocksView renders real GLM wire across all block types', async ({ page }) => {
  await page.goto('/chat.html');

  // text block — GLM's final answer (Markdown-rendered). The recorded run
  // replied: The package name is "dev-workbench".
  await expect(page.getByTestId('chat-block-text')).toContainText('dev-workbench');

  // tool_use head — the read_file call GLM made against Cargo.toml.
  await expect(page.getByTestId('chat-block-tool-name')).toHaveText('read_file');

  // thinking / tool_result / result heads all render — the real block types the
  // live trace produced, not a curated subset. GLM interleaves thinking around
  // the tool call (a trace before AND after read_file), so normalize yields
  // multiple thinking blocks — assert at least one, not a fixed count (the live
  // trace shape varies per run).
  await expect(page.getByTestId('chat-block-thinking').first()).toContainText('思考过程');
  await expect(page.getByTestId('chat-block-toolresult')).toContainText('工具结果');
  await expect(page.getByTestId('chat-block-result')).toContainText('完成');

  // Expand the tool_result card — the REAL Cargo.toml bytes GLM received must
  // render into the DOM, proving genuine tool output flows through the wire.
  await page.getByTestId('chat-block-toolresult-head').click();
  await expect(page.getByTestId('chat-block-toolresult-content')).toContainText('name = "dev-workbench"');
});
