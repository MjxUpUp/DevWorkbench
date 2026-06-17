import { test, expect, type Page } from '@playwright/test';

/**
 * Frontend E2E for the three Settings ↔ backend gaps this task closes:
 *   1. providers — model context window now editable + persisted (v2.0 fix)
 *   2. memory    — long-term memory flywheel list + delete (v1.3-T2 surface)
 *   3. skills    — registry list + catalog install (skills subsystem surface)
 *
 * Real browser, real production components (the only mock is the Tauri IPC
 * boundary). Each test seeds the commands its section needs and asserts on the
 * recorded __INVOKE_CALLS__ that the real Rust commands would have received.
 */

async function seedMock(page: Page, mock: Record<string, unknown>) {
  await page.addInitScript((m) => {
    (window as any).__MOCK_INVOKE__ = m;
  }, mock);
}

async function calls(page: Page): Promise<{ cmd: string; args: any }[]> {
  return page.evaluate(() => (window as any).__INVOKE_CALLS__ as { cmd: string; args: any }[]);
}

const EMPTY_PROVIDERS = { providers: [], modelMapping: {} };
const EMPTY: unknown[] = [];

test('providers: editing a model context window persists through save', async ({ page }) => {
  await seedMock(page, {
    get_providers_config: {
      providers: [
        {
          id: 'zai',
          name: 'ZAI',
          endpoint: 'https://x.test',
          apiKey: 'sk-key',
          enabled: true,
          models: [{ id: 'glm-4.6', label: 'GLM-4.6', enabled: true }],
        },
      ],
      modelMapping: {},
    },
    set_providers_config: null,
    // Other sections mount simultaneously — empty responses keep them quiet.
    get_knowledge_for_project: EMPTY,
    list_skills: EMPTY,
    skill_catalog: EMPTY,
  });
  await page.goto('/settings.html');

  // The single model row's context-window input — fill a real window.
  await page.locator('input[aria-label="上下文窗口"]').fill('128000');
  await page.getByRole('button', { name: '保存全部更改' }).click();

  const all = await calls(page);
  const save = all.find((c) => c.cmd === 'set_providers_config');
  expect(save, 'set_providers_config must fire on save').toBeTruthy();
  expect(save!.args.config.providers[0].models[0].contextWindow).toBe(128000);
});

test('memory: lists project entries and deletes one on click', async ({ page }) => {
  await seedMock(page, {
    get_providers_config: EMPTY_PROVIDERS,
    get_knowledge_for_project: [
      {
        id: 'k1',
        projectHash: 'h',
        category: 'memory',
        title: 'root cause x',
        content: 'the real root cause',
        sourceAgent: 'react_kernel',
        sourceSessionId: null,
        sourceType: 'react_session',
        confidence: 0.9,
        createdAt: '2026-06-17T00:00:00Z',
        updatedAt: '2026-06-17T00:00:00Z',
        accessCount: 0,
      },
    ],
    delete_knowledge_entry: null,
    list_skills: EMPTY,
    skill_catalog: EMPTY,
  });
  await page.goto('/settings.html');

  await expect(page.getByText('root cause x')).toBeVisible();
  await page.getByRole('button', { name: /删除记忆 root cause x/ }).click();

  const all = await calls(page);
  const del = all.find((c) => c.cmd === 'delete_knowledge_entry');
  expect(del, 'delete_knowledge_entry must fire').toBeTruthy();
  expect(del!.args).toMatchObject({ id: 'k1' });
});

test('skills: lists installed skills and installs a catalog entry', async ({ page }) => {
  await seedMock(page, {
    get_providers_config: EMPTY_PROVIDERS,
    get_knowledge_for_project: EMPTY,
    list_skills: [
      {
        id: 's1',
        org: 'local',
        name: 'my-skill',
        description: 'an installed skill',
        category: 'tool',
        rating: 4,
        installedAt: '2026-06-17T00:00:00Z',
      },
    ],
    skill_catalog: [
      {
        name: 'fresh-skill',
        description: 'a discoverable skill',
        source: '/home/.agents/skills/fresh-skill',
        scope: 'global',
      },
    ],
    install_skill_from_catalog: {
      id: 's2',
      org: 'local',
      name: 'fresh-skill',
      description: 'a discoverable skill',
      category: null,
      rating: null,
      installedAt: '2026-06-17T00:00:00Z',
    },
  });
  await page.goto('/settings.html');

  // Installed skill is listed.
  await expect(page.getByText('my-skill')).toBeVisible();
  // Install the catalog entry.
  await page.getByRole('button', { name: '安装技能 fresh-skill' }).click();

  const all = await calls(page);
  const install = all.find((c) => c.cmd === 'install_skill_from_catalog');
  expect(install, 'install_skill_from_catalog must fire').toBeTruthy();
  expect(install!.args).toMatchObject({
    name: 'fresh-skill',
    source: '/home/.agents/skills/fresh-skill',
  });
});
