import { test, expect, type Page } from '@playwright/test';

/**
 * Frontend E2E for the v1.2 T6 shadow-git rollback button (FileChanges.tsx).
 * Real browser, real React render, real click + confirm dialog. Only the Tauri
 * IPC boundary is mocked (unavoidable outside the Tauri runtime); the component
 * under test is the production code.
 */

// Seed the mocked invoke responses BEFORE the harness's module entry runs.
// addInitScript runs before any page script, so window.__MOCK_INVOKE__ is set
// before FileChanges mounts and probes get_checkpoint.
async function seedMock(page: Page, mock: Record<string, unknown>) {
  await page.addInitScript((m) => {
    (window as any).__MOCK_INVOKE__ = m;
  }, mock);
}

test('rollback button shows with a checkpoint and rolls back on confirm', async ({ page }) => {
  await seedMock(page, {
    get_checkpoint: { sessionId: 's1', headSha: 'abc123' },
    rollback_to_checkpoint: { restoredFiles: ['a.rs'], removedUntracked: [], skipped: [] },
  });
  await page.goto('/');

  // The changed-files list renders.
  await expect(page.getByText('a.rs')).toBeVisible();
  await expect(page.getByText('b.ts')).toBeVisible();

  // The rollback button appears because get_checkpoint returned a checkpoint.
  const rollbackBtn = page.getByRole('button', { name: /回滚改动/ });
  await expect(rollbackBtn).toBeVisible();

  // Accept the confirm() dialog, then click.
  page.once('dialog', (d) => d.accept());
  await rollbackBtn.click();

  // The success summary renders.
  await expect(page.getByText(/已回滚/)).toBeVisible();

  // And the rollback command was actually invoked with the right args.
  const calls = await page.evaluate(() => (window as any).__INVOKE_CALLS__ as { cmd: string; args: any }[]);
  const rollbackCall = calls.find((c) => c.cmd === 'rollback_to_checkpoint');
  expect(rollbackCall, 'rollback_to_checkpoint must be invoked').toBeTruthy();
  expect(rollbackCall!.args).toMatchObject({ sessionId: 's1', force: false });
});

test('rollback button is hidden when no checkpoint exists', async ({ page }) => {
  await seedMock(page, { get_checkpoint: null });
  await page.goto('/');

  await expect(page.getByText('a.rs')).toBeVisible();
  await expect(page.getByRole('button', { name: /回滚改动/ })).toBeHidden();
});

test('cancel the confirm dialog performs no rollback', async ({ page }) => {
  await seedMock(page, { get_checkpoint: { sessionId: 's1', headSha: 'abc' } });
  await page.goto('/');

  const rollbackBtn = page.getByRole('button', { name: /回滚改动/ });
  await expect(rollbackBtn).toBeVisible();

  // Dismiss the confirm → no rollback call.
  page.once('dialog', (d) => d.dismiss());
  await rollbackBtn.click();

  // The file list is still showing (result summary did NOT replace it).
  await expect(page.getByText('a.rs')).toBeVisible();

  const calls = await page.evaluate(() => (window as any).__INVOKE_CALLS__ as { cmd: string }[]);
  expect(
    calls.some((c) => c.cmd === 'rollback_to_checkpoint'),
    'no rollback call after cancel',
  ).toBeFalsy();
});
