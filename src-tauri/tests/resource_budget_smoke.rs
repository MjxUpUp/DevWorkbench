//! Smoke test for `kernel_impl::resource_budget::classify_task_kind` —
//! specifically the review P0-1 fix (single-char `改`/`修`/`加` removed from
//! multi_edit_kw in favor of 2+ char collocations). This file exists so the
//! regression is checkable via the integration-test binary path (which
//! sidesteps the dev-workbench Windows 0xc0000139 lib-test startup issue,
//! per memory `applib-test-binary-entrypoint-block.md`). The matching unit
//! tests in `resource_budget.rs::tests` cover the same surface area and
//! exercise on macOS/Linux CI.

use app_lib::kernel_impl::resource_budget::{classify_task_kind, TaskKind};

#[test]
fn classify_改主意_no_false_positive_long_running() {
    // Review P0-1: this used to be classified as MultiEdit because the
    // single-char "改" was in multi_edit_kw. After the fix it falls through
    // to LongRunning (the safest over-budget default).
    assert_eq!(classify_task_kind("我改主意了"), TaskKind::LongRunning);
}

#[test]
fn classify_改用_switches_long_running() {
    // "改用另一种方案" — "改" present but context is "switch to another
    // approach", not file edit. P0-1 fix routes this to LongRunning.
    assert_eq!(classify_task_kind("改用另一种方案"), TaskKind::LongRunning);
}

#[test]
fn classify_改成_real_edit_still_multi_edit() {
    // Counter-test: the 2+ char collocation `改成` is in multi_edit_kw, so
    // genuine edit intent still routes to MultiEdit.
    assert_eq!(classify_task_kind("把这段代码改成异步"), TaskKind::MultiEdit);
    assert_eq!(classify_task_kind("改成 ESM"), TaskKind::MultiEdit);
    assert_eq!(classify_task_kind("修改一下配置"), TaskKind::MultiEdit);
    assert_eq!(classify_task_kind("修复 docs/issue/foo.md 里的 bug"), TaskKind::MultiEdit);
}

#[test]
fn classify_改一下_still_multi_edit() {
    // "改一下" is the 2-char collocation that captures the most common Chinese
    // edit phrasing. Pins that the tightening didn't regress real-edit detection.
    assert_eq!(classify_task_kind("改一下这个函数"), TaskKind::MultiEdit);
}

#[test]
fn classify_english_fix_still_multi_edit() {
    // English keywords unchanged from previous behavior.
    assert_eq!(classify_task_kind("please fix this bug"), TaskKind::MultiEdit);
    assert_eq!(classify_task_kind("apply the patch"), TaskKind::MultiEdit);
}

#[test]
fn classify_refactor_unchanged() {
    // Refactor keywords weren't touched by P0-1 fix.
    assert_eq!(classify_task_kind("重构这个模块"), TaskKind::Refactor);
    assert_eq!(classify_task_kind("refactor this"), TaskKind::Refactor);
}

#[test]
fn classify_readonly_unchanged() {
    // ReadOnly keywords weren't touched.
    assert_eq!(classify_task_kind("列出项目里的文件"), TaskKind::ReadOnly);
    assert_eq!(classify_task_kind("find all TODO comments"), TaskKind::ReadOnly);
}

#[test]
fn classify_empty_long_running() {
    assert_eq!(classify_task_kind(""), TaskKind::LongRunning);
    assert_eq!(classify_task_kind("你好"), TaskKind::LongRunning);
}

#[test]
fn classify_加_word_question_no_false_positive() {
    // Review P0-1: "加" alone is no longer in multi_edit_kw. A yes/no
    // question containing "加" should fall through to LongRunning, not
    // MultiEdit. (Comment in the unit test had this same expectation.)
    assert_eq!(classify_task_kind("我加不加这个文件？"), TaskKind::LongRunning);
}

#[test]
fn classify_加上_real_edit_still_multi_edit() {
    // Counter-test: "加上" (a 2-char collocation that captures real edit
    // intent) is still in multi_edit_kw, so genuine add-edit still wins.
    assert_eq!(classify_task_kind("加上 ESLint 配置"), TaskKind::MultiEdit);
    assert_eq!(classify_task_kind("添加 error handling"), TaskKind::MultiEdit);
}
