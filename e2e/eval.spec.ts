import { test, expect } from '@playwright/test';

/**
 * Eval/Replay panel front-end E2E. EvalPanel runs in a real browser against
 * the recorded-shape rows the back-end eval::cases::list_eval_cases /
 * eval::verdicts::list_verdicts / eval::runs::eval_trend return, served through
 * the real IPC boundary shape (window.__MOCK_INVOKE__). Paired with the back-end
 * pure-core tests (score_replay / compare_paired / extract_trajectory), this
 * closes the loop: store → invoke → render without spinning up the Tauri app or
 * contacting a provider.
 *
 * The point of the feature is making the three anti-gaming principles VISIBLE:
 *  - 客观事实代码判 — P1 lists ready + probe cases; P2 shows input_prompt
 *    LOCKED (C1: the conversation record is read-only) while contract fields
 *    stay editable.
 *  - 因果归因 — V1 ledger renders CLEAR + BRAKE attribution badges across
 *    eval/honesty gates, and the gate filter narrows rows.
 *  - 配对回放 — P5 paired compare renders old(0.5 FAIL/BRAKE) vs new(0.9
 *    PASS/CLEAR) and surfaces 净提升 (net improve). This is also the
 *    regression guard for the oldV/newV label-swap bug that inverted the
 *    brake/admit verdict.
 */
test('EvalPanel surfaces locked prompt, attribution badges, paired net-improve, 4 real eval drivers', async ({
  page,
}) => {
  await page.goto('/eval.html');

  // ── P1 default: both cases render (ready + probe). ──
  await expect(page.getByText('修复 BlocksView tool_use 切分')).toBeVisible();
  await expect(page.getByText('compaction 跨模型归档')).toBeVisible();

  // ── 客观事实 / C1: P2 detail — input_prompt LOCKED, name EDITABLE. ──
  await page.getByText('修复 BlocksView tool_use 切分').click();
  await expect(page.getByTestId('eval-feature-title')).toHaveText('Case 详情 / 编辑');
  // The prompt renders as a read-only text node (not an input the agent could
  // rewrite to cover its tracks).
  await expect(page.getByText('edit 工具一直调用中，帮我排查')).toBeVisible();
  // The name, by contrast, is an editable textbox (the first one in P2)
  // carrying the current value — distinct from the locked prompt text node.
  await expect(page.getByRole('textbox').first()).toHaveValue('修复 BlocksView tool_use 切分');

  // ── 因果归因: V1 ledger — CLEAR + BRAKE badges across eval/honesty gates. ──
  await page.getByTestId('eval-nav-V1').click();
  await expect(page.getByTestId('eval-feature-title')).toHaveText('Verdicts 查询');
  const table = page.getByTestId('eval-verdict-table');
  // Two eval rows (the new + old runs — the paired-compare data source) and
  // one honesty row.
  await expect(table.getByText('eval')).toHaveCount(2);
  await expect(table.getByText('honesty')).toHaveCount(1);
  // One CLEAR (the PASS eval) + two BRAKE (the FAIL eval + the honesty FAIL).
  await expect(table.getByText('CLEAR', { exact: true })).toHaveCount(1);
  await expect(table.getByText('BRAKE', { exact: true })).toHaveCount(2);

  // Gate filter narrows: honesty only → both eval rows leave the table.
  await page.getByRole('combobox').first().selectOption('honesty');
  await expect(table.getByText('eval')).toHaveCount(0);
  await expect(table.getByText('honesty')).toHaveCount(1);

  // ── 配对回放: P5 — new run (0.9 PASS/CLEAR) beat old (0.5 FAIL/BRAKE) → 净提升. ──
  await page.getByTestId('eval-nav-P5').click();
  await expect(page.getByText('净提升 · 可准入')).toBeVisible();
  await expect(page.getByText('回归 · 拦')).toHaveCount(0);

  // ── 3 evaluation objects (P4). Both remaining platform objects are REAL
  //    drivers: e2e (in-memory DB + real logic, no LLM) / 加持 (skills OFF→ON
  //    paired, live key in prod — shim-served here). ──
  await page.getByTestId('eval-nav-P4').click();
  await expect(page.getByText('平台-e2e')).toBeVisible();
  await expect(page.getByText('平台-加持')).toBeVisible();
  // 平台-e2e → real runner (data plane, no LLM): 运行 e2e 评测 → PASS + checks.
  await page.locator('label', { hasText: '平台-e2e' }).click();
  await expect(page.getByRole('button', { name: /运行 e2e 评测/ })).toBeVisible();
  await expect(page.getByText(/需平台评测驱动/)).toHaveCount(0);
  await page.getByRole('button', { name: /运行 e2e 评测/ }).click();
  await expect(page.getByText(/项检查/)).toBeVisible();
  // 平台-加持 → real runner (skills OFF→ON paired): 运行加持评测 → CLEAR +
  // improvement + off→on score delta. (Working dir comes from activeProject.path
  // set in eval-main; run_eval_enablement is shim-served — the real driver needs
  // a provider key for its two live agents.)
  await page.locator('label', { hasText: '平台-加持' }).click();
  await expect(page.getByRole('button', { name: /运行加持评测/ })).toBeVisible();
  await expect(page.getByText(/需平台评测驱动/)).toHaveCount(0);
  await page.getByRole('button', { name: /运行加持评测/ }).click();
  await expect(page.getByText('CLEAR', { exact: true })).toBeVisible();
  await expect(page.getByText('improvement')).toBeVisible();

  // ── P6: the 8-dim AgentX reliability rubric + weighted Q_code render. The
  //    newest eval verdict (v-eval-new) carries session_id + case_id, so
  //    RubricCard assembles the rubric via score_eval_rubric. Locks the gap
  //    closure: a prior version showed a 3-row fake rubric + "needs scoring.rs
  //    extension" note; the backend now computes all 8 dims. ──
  await page.getByTestId('eval-nav-P6').click();
  await expect(page.getByText(/Q_code/)).toBeVisible();
  // All 8 dimension labels render (S-tier hallucination + hard-gate manual).
  await expect(page.getByText('attribute hallucination')).toBeVisible();
  await expect(page.getByText('manual intervention ⚠硬门')).toBeVisible();
  await expect(page.getByText('harness-pattern')).toBeVisible();
  await expect(page.getByText('dryrun pass')).toBeVisible();

  // ── SA 平台自审: F (前端 invoke 集合) vs B (后端 generate_handler! 注册) 对齐。
  //    CoverageSelfAudit 把构建期生成的 INVOKED_COMMANDS manifest 真展开传入 IPC ——
  //    这是单测 mock evalApi 拿不到的信号（单测不验证 manifest 内容真接线）。零死按钮
  //    → PASS；死代码（后端注册前端没调）→ WARN 区可见。死按钮 FAIL 形态由单测覆盖。 ──
  await page.getByTestId('eval-nav-SA').click();
  await expect(page.getByTestId('eval-feature-title')).toContainText('IPC 接线自审');
  await expect(page.getByTestId('coverage-verdict')).toContainText('PASS');
  // 死代码 WARN 区渲染（后端注册了前端没调的 command）。
  await expect(page.getByTestId('coverage-dead-code')).toBeVisible();
  // 零死按钮 → 死按钮 FAIL 区不渲染。
  await expect(page.getByTestId('coverage-dead-buttons')).toHaveCount(0);
});
