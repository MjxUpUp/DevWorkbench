/**
 * Visual Regression Tests (Layer 1) — DevWorkbench 视觉回归基线。
 *
 * 设计依据（调研结论，见 ai-collab-diagnosis-20260620-1327/dive_06.md）：
 * - 当前 e2e/ 全是功能断言（textContent /toBeVisible/流式渲染），零视觉回归。
 *   e2e-test-plan.md 的 4.1 "zcode 美学验证"（SVG 图标非 emoji、当前项高亮）还在靠人眼。
 * - 像素对比在"组件级 + 固定 CI 环境 + 合理阈值"下基本可控（Reddit 共识），
 *   全页面/跨浏览器才 flaky。本文件只做组件级。
 * - 多模态 LLM-judge 不能当 CI gate（deviate up to 5 points，加 CoT 暴跌），
 *   只作辅助；本文件是确定性 Layer 1，不依赖 LLM。
 *
 * 使用：
 *   npx playwright test visual.spec.ts --update-snapshots   # 首次/基线变更
 *   npx playwright test visual.spec.ts                       # 回归对比
 *
 * 基线变更纪律：--update-snapshots 产生的 diff 必须走 PR review，
 * 不能在 CI 里自动更新（否则视觉回归形同虚设）。
 */
import { test, expect } from '@playwright/test';

// 组件级截图：每个独立挂载点一张，避免全页面 flaky。
// 阈值：1% 像素差异容忍（字体抗锯齿/亚像素渲染微差），超过即报 diff。
const VISUAL = { maxDiffPixelRatio: 0.01, animations: 'disabled' as const };

test.describe('Visual regression — component baselines', () => {
  test.beforeEach(async ({ page }) => {
    // 拦截可能引入非确定性的动态内容（时间戳/随机头像/网络图）。
    // 真实 GLM wire 由 fixtures 提供，不走网络。
    await page.route('**/*.(png|jpg|svg)', (route) => {
      // 静态资源正常放行；外部随机图按需在此 mask
      route.continue();
    });
  });

  test('app shell — 初始空状态视觉基线', async ({ page }) => {
    await page.goto('/app.html');
    // 等渲染稳定（stores hydrate + 首帧 paint）
    await expect(page.locator('#root')).toBeVisible();
    await page.waitForTimeout(300);
    // 只截主交互区，不含会漂移的窗口控制条
    await expect(page).toHaveScreenshot('app-shell.png', VISUAL);
  });

  test('settings — sections 视觉基线', async ({ page }) => {
    // 截 settings.html harness 实际挂载的 5 个 section（providers/memory/skills/hooks/capability）。
    // 注：左侧导航 (.settings-view-nav) 属 SettingsView 外壳，当前 harness 未挂载——
    // 这是既有设计取舍（section harness 服务功能测试 settings.spec.ts）。
    // nav 视觉基线待补 dedicated full-shell harness（重构 SettingsView 时一并补）。
    // settings.html 依赖 __MOCK_INVOKE__ 注入，否则 5 个 section 调 invoke 返回 null 渲染失败。
    await page.addInitScript(() => {
      (window as any).__MOCK_INVOKE__ = {
        get_providers_config: { providers: [], modelMapping: {} },
        get_knowledge_for_project: [],
        list_skills: [],
        skill_catalog: [],
        list_user_hooks: [],
        mcp_catalog: [],
      };
    });
    await page.goto('/settings.html');
    await expect(page.locator('[data-e2e="providers"]').first()).toBeVisible();
    await page.waitForTimeout(300);
    await expect(page).toHaveScreenshot('settings-sections.png', VISUAL);
  });

  test('chat — 空对话状态视觉基线', async ({ page }) => {
    await page.goto('/chat.html');
    await expect(page.locator('#root')).toBeVisible();
    await page.waitForTimeout(300);
    await expect(page).toHaveScreenshot('chat-empty.png', VISUAL);
  });
});

test.describe('Visual regression — 关键交互态', () => {
  // 交互态截图比静态态更易 flaky（动画/过渡），单独分组，阈值略宽
  const VISUAL_INTERACTIVE = { maxDiffPixelRatio: 0.02, animations: 'disabled' as const };

  test('trigger — 触发器面板激活态', async ({ page }) => {
    await page.goto('/trigger.html');
    await expect(page.locator('#root')).toBeVisible();
    // 若有激活态按钮，点击进入；否则用默认态
    const activator = page.locator('[data-testid="activate"], button:has-text("激活"), button:has-text("启用")').first();
    if (await activator.count()) await activator.click();
    await page.waitForTimeout(400); // 过渡动画
    await expect(page).toHaveScreenshot('trigger-active.png', VISUAL_INTERACTIVE);
  });
});
