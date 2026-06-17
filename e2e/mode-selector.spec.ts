import { test, expect } from '@playwright/test';

/**
 * Frontend E2E for the v2.0 C6 dry-run execution mode (ModeSelector.tsx).
 * Real browser, real React render, real click on the dropdown. The production
 * ModeSelector is the component under test; the harness just captures onChange.
 *
 * Proves the dry-run option is exposed to the user and selecting it yields the
 * 'dry-run' wire value that spawn_agent_session deserializes into
 * PermissionMode::DryRun (verified on the Rust side by the execute_tool_call
 * unit tests).
 */

test('dry-run option renders and is selectable', async ({ page }) => {
  await page.goto('/mode-selector.html');

  // Open the dropdown (trigger button shows the current mode's short label).
  await page.locator('.mode-selector-trigger').click();

  // The dry-run option ("预演") is present — the v2.0 C6 frontend addition.
  const dryRunOption = page.getByRole('option', { name: /预演/ });
  await expect(dryRunOption).toBeVisible();

  // Selecting it fires onChange('dry-run').
  await dryRunOption.click();
  const selected = await page.evaluate(() => (window as any).__MODE_CHANGE__);
  expect(selected).toBe('dry-run');
});

test('all six modes are offered in the dropdown', async ({ page }) => {
  await page.goto('/mode-selector.html');
  await page.locator('.mode-selector-trigger').click();

  // Every mode must be reachable — guards against an accidental truncation of
  // the option list when dry-run was inserted. Scope to the dropdown (the
  // trigger also shows the active label) and match the label span EXACTLY: a
  // regex would also hit the 预演 option whose desc contains "计划".
  const dropdown = page.locator('.mode-selector-dropdown');
  for (const label of ['默认', '自动', '计划', '预演', '静默', '跳过']) {
    await expect(dropdown.getByText(label, { exact: true })).toBeVisible();
  }
});
