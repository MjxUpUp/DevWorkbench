import { test, expect, type Page } from '@playwright/test';

/**
 * Frontend E2E for the `$` trigger reading real installed skills.
 *
 * Before this fix the `$` menu rendered PLACEHOLDER_SKILLS (4 hardcoded fake
 * items: 新建功能/代码重构/性能优化/安全审计). Now it async-loads list_skills
 * — the same backend command the settings SkillsSection uses — so the menu is
 * sourced from the real Skill registry the kernel resolves against.
 *
 * Real browser, real production TriggerMenu; only the Tauri IPC boundary is
 * mocked. Asserts the real skill names render and list_skills was invoked.
 */

async function seedMock(page: Page, mock: Record<string, unknown>) {
  await page.addInitScript((m) => {
    (window as any).__MOCK_INVOKE__ = m;
  }, mock);
}

async function calls(page: Page): Promise<{ cmd: string; args: any }[]> {
  return page.evaluate(() => (window as any).__INVOKE_CALLS__ as { cmd: string; args: any }[]);
}

test('$ trigger lists real installed skills via list_skills', async ({ page }) => {
  await seedMock(page, {
    list_skills: [
      {
        id: 's1',
        org: 'local',
        name: 'pdf-extract',
        description: 'extract text from pdf files',
        category: 'tool',
        rating: 4,
        installedAt: '2026-06-17T00:00:00Z',
      },
      {
        id: 's2',
        org: 'local',
        name: 'commit-helper',
        description: 'draft conventional commit messages',
        category: 'tool',
        rating: 5,
        installedAt: '2026-06-17T00:00:00Z',
      },
    ],
  });
  await page.goto('/trigger.html');

  // Real installed-skill names render (not the old placeholders).
  await expect(page.getByText('pdf-extract')).toBeVisible();
  await expect(page.getByText('commit-helper')).toBeVisible();

  // The menu actually queried the backend.
  const all = await calls(page);
  const list = all.find((c) => c.cmd === 'list_skills');
  expect(list, 'list_skills must fire on $ trigger').toBeTruthy();
});

test('$ trigger falls back to empty list when list_skills fails', async ({ page }) => {
  await seedMock(page, {
    list_skills: () => Promise.reject(new Error('backend down')),
  });
  await page.goto('/trigger.html');

  // No crash, no placeholder fakes — empty state shows.
  await expect(page.getByText('无匹配结果')).toBeVisible();
  await expect(page.getByText('新建功能')).toHaveCount(0);
});
