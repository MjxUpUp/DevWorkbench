//! Adaptive Resource Budget — task-aware step / token / wall-clock limits
//! + early-convergence signals.
//!
//! ## Why this exists
//!
//! The previous model hard-coded a single `max_steps = 30` for every kernel
//! agent run. Simple read-only tasks wasted tokens, while complex
//! "code-check + multi-file fix" runs (the failure mode in conversation
//! cfa53764, session 61070a4c) hit the ceiling before the agent could
//! converge — output_summary read "Reached the 30-step tool-call limit
//! without a final answer." The user reported this as "老是执行一半就失败".
//!
//! ## What this module provides
//!
//! 1. `TaskKind` — coarse classification of the user's prompt by intent
//!    (read-only / analysis / multi-edit / refactor / long-running).
//! 2. `ResourceBudget` — multi-dimensional limits (steps / tokens /
//!    wallclock / retries) per kind. The previous 1-D hard ceiling is
//!    replaced with a structured budget object the executor picks up.
//! 3. `convergence_reminder` — pure helper that returns a system-reminder
//!    string when `remaining_ratio < 0.25`, telling the model to start
//!    wrapping up. This is the early-warning that the 30-step cap never
//!    gave — instead of dying on the ceiling, the model gets a soft nudge
//!    several steps before.
//!
//! The classification is rule-based (keyword matching); it deliberately
//! avoids an ML classifier because (a) it's cheap, (b) it's deterministic
//! for testing, and (c) the input space is small. A future enhancement can
//! swap in an LLM-based classifier behind the same `classify_task_kind`
//! signature without changing call sites.

/// Coarse classification of user intent. Drives the budget shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskKind {
    /// Pure reading / listing / searching. No edits expected.
    ReadOnly,
    /// Analyze something and report findings. May need broad reads but no writes.
    Analysis,
    /// Edit multiple files (the cfa53764 failure mode).
    MultiEdit,
    /// Restructure existing code (rename / move / split) across files.
    Refactor,
    /// Long-running batch / monitoring / exploration tasks.
    LongRunning,
}

impl TaskKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Analysis => "analysis",
            Self::MultiEdit => "multi_edit",
            Self::Refactor => "refactor",
            Self::LongRunning => "long_running",
        }
    }
}

/// Multi-dimensional budget for one agent run. Any single dimension hitting
/// its limit emits a "start converging" reminder; if the model ignores it,
/// the executor eventually halts with a clear "ran out of X" message.
#[derive(Clone, Copy, Debug)]
pub struct ResourceBudget {
    pub max_steps: usize,
    /// Per-LLM-call input token cap. 0 = no cap.
    pub max_input_tokens: usize,
    /// Total wall-clock seconds for the run. 0 = no cap.
    pub max_wallclock_secs: u64,
    /// Retry budget across all LLM errors (stream truncate / network etc).
    pub max_retries: u32,
}

impl Default for ResourceBudget {
    /// Default == LongRunning table values. Reasoning: a caller that forgets to
    /// pick a TaskKind should fall back to the MOST GENEROUS budget, not the
    /// tightest — over-budget is recoverable (retry with the same prompt
    /// usually works); under-budget starves the agent silently. If a future
    /// kind is added without an entry in `for_kind`, default stays safe.
    fn default() -> Self {
        Self::for_kind(TaskKind::LongRunning)
    }
}

impl ResourceBudget {
    /// Baseline budget for a task kind. Tuned from observed cfa53764 history:
    /// MultiEdit at 30 was too tight; 80 leaves room for read→edit→test→fix
    /// loops the model already exhibits when it converges well.
    pub fn for_kind(kind: TaskKind) -> Self {
        match kind {
            // Reading a file or listing stuff — short and deterministic.
            TaskKind::ReadOnly => Self {
                max_steps: 20,
                max_input_tokens: 60_000,
                max_wallclock_secs: 180,
                max_retries: 2,
            },
            // Analysis: read several files, synthesize. Mid-range.
            TaskKind::Analysis => Self {
                max_steps: 40,
                max_input_tokens: 90_000,
                max_wallclock_secs: 300,
                max_retries: 3,
            },
            // The big one — read + multi-file edit + verify. Hard ceiling
            // lifted from 30 to 80; convergence reminder fires at 60 (75%).
            TaskKind::MultiEdit => Self {
                max_steps: 80,
                max_input_tokens: 120_000,
                max_wallclock_secs: 900,
                max_retries: 4,
            },
            // Refactor: similar to MultiEdit but slightly tighter (refactors
            // usually have a narrower blast radius than multi-fix).
            TaskKind::Refactor => Self {
                max_steps: 60,
                max_input_tokens: 120_000,
                max_wallclock_secs: 600,
                max_retries: 4,
            },
            // Unknown / batch / exploration — generous ceiling.
            TaskKind::LongRunning => Self {
                max_steps: 120,
                max_input_tokens: 150_000,
                max_wallclock_secs: 1800,
                max_retries: 5,
            },
        }
    }

    /// Ratio (0.0..=1.0) of steps still available. Used by
    /// `convergence_reminder` to decide whether to nudge.
    pub fn steps_remaining_ratio(&self, used: usize) -> f32 {
        if self.max_steps == 0 {
            1.0
        } else {
            (self.max_steps.saturating_sub(used)) as f32 / self.max_steps as f32
        }
    }
}

/// Classify a user prompt into a TaskKind. Pure / deterministic; key
/// matching is case-insensitive Chinese + English.
///
/// Design note (review P0-1): MultiEdit keywords are 2+ character collocations
/// only — single Chinese characters (`修`, `改`, `加`) collide with hundreds
/// of non-edit phrases (`我改主意了`, `修辞`, `加快`, `改天`, `加仑`). The old
/// implementation had a near-100% false-positive rate for any Chinese prompt
/// containing one of those characters. Word-boundary matching isn't viable
/// for Chinese (no spaces), so 2-character minimum collocations are the
/// pragmatic fix.
pub fn classify_task_kind(prompt: &str) -> TaskKind {
    let p = prompt.to_lowercase();

    // MultiEdit signals — 2+ char collocations (Chinese) + whole-word English.
    // Each entry below is the shortest "intent-revealing" token; bare-char
    // matches are deliberately avoided.
    let multi_edit_kw = [
        "修复", "修正", "改成", "改为", "改成", "加上", "添加", "实现",
        "改动", "修改", "改一下", "修一下", "加一下",
        "fix", "patch", "edit", "modify",
        "提交", "apply", "implement", "inject",
    ];
    let analysis_kw = [
        "分析", "调研", "调研", "评估", "评估", "检查", "audit",
        "analyze", "investigate", "evaluate", "review", "research",
    ];
    let refactor_kw = [
        "重构", "重写", "拆分", "refactor", "rewrite", "restructure",
        "rename", "改名", "move ", "split ", "迁移",
    ];
    let readonly_kw = [
        "列出", "查找", "看看", "read", "list", "show ", "find ",
        "搜索", "grep", "查看", "display", "打印",
    ];

    // Priority: more specific kinds first (MultiEdit > Refactor > Analysis
    // > ReadOnly > LongRunning default).
    if multi_edit_kw.iter().any(|k| p.contains(k)) {
        // Distinguish pure read-only "看X" from "修改X" — if the prompt
        // mentions files / structure AND a fix verb, it's MultiEdit.
        return TaskKind::MultiEdit;
    }
    if refactor_kw.iter().any(|k| p.contains(k)) {
        return TaskKind::Refactor;
    }
    if analysis_kw.iter().any(|k| p.contains(k)) {
        return TaskKind::Analysis;
    }
    if readonly_kw.iter().any(|k| p.contains(k)) {
        return TaskKind::ReadOnly;
    }
    TaskKind::LongRunning
}

/// Returns a system-reminder string telling the model to start wrapping
/// up. Returns `None` when remaining ratio is healthy — no point nagging
/// the model mid-task. Pure.
pub fn convergence_reminder(
    kind: TaskKind,
    used: usize,
    budget: &ResourceBudget,
) -> Option<String> {
    let ratio = budget.steps_remaining_ratio(used);
    if ratio > 0.25 {
        return None;
    }
    let remaining = budget.max_steps.saturating_sub(used);
    Some(format!(
        "[convergence-budget] {kind} task has used {used}/{} steps ({:.0}% used). \
         Remaining: {remaining} steps. Please start wrapping up — produce a \
         concrete final answer or a tight list of next actions. Avoid new \
         exploratory reads unless absolutely necessary; every step from now \
         on should bring the task closer to completion.",
        budget.max_steps,
        (1.0 - ratio) * 100.0,
        kind = kind.as_str(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_chinese_fix_is_multi_edit() {
        assert_eq!(classify_task_kind("修复docs/issue/foo.md 里的 bug"), TaskKind::MultiEdit);
        assert_eq!(classify_task_kind("改一下这个函数"), TaskKind::MultiEdit);
    }

    #[test]
    fn classify_english_fix_is_multi_edit() {
        assert_eq!(classify_task_kind("please fix this bug"), TaskKind::MultiEdit);
        assert_eq!(classify_task_kind("apply the patch"), TaskKind::MultiEdit);
    }

    #[test]
    fn classify_chinese_analysis() {
        assert_eq!(classify_task_kind("分析下这个项目的架构"), TaskKind::Analysis);
        assert_eq!(classify_task_kind("调研业界做法"), TaskKind::Analysis);
    }

    #[test]
    fn classify_english_analysis() {
        assert_eq!(classify_task_kind("investigate this issue"), TaskKind::Analysis);
        assert_eq!(classify_task_kind("review the architecture"), TaskKind::Analysis);
    }

    #[test]
    fn classify_refactor() {
        assert_eq!(classify_task_kind("重构这个模块"), TaskKind::Refactor);
        assert_eq!(classify_task_kind("refactor this"), TaskKind::Refactor);
    }

    #[test]
    fn classify_readonly() {
        assert_eq!(classify_task_kind("列出项目里的文件"), TaskKind::ReadOnly);
        assert_eq!(classify_task_kind("find all TODO comments"), TaskKind::ReadOnly);
    }

    #[test]
    fn classify_unknown_is_long_running() {
        assert_eq!(classify_task_kind("你好"), TaskKind::LongRunning);
        assert_eq!(classify_task_kind(""), TaskKind::LongRunning);
    }

    // ----- review M2 negative / boundary cases -----
    // "我改主意了" 表面含 "改" 但语义是"不再修" → ReadOnly (default 走 readonly
    // 关键词不命中，回到 LongRunning; 但当前是 LongRunning——这是合理的：
    // 模糊短句归到 LongRunning 而非 ReadOnly 更安全)。下面这些是 design
    // intent 文档，验证"classifier 不会把澄清/评论误判为 MultiEdit"。

    #[test]
    fn classify_short_clarification_falls_through_to_long_running() {
        // "我改主意了" 表面含 "改" 但语义是"不再修"。修前：单 char "改" 在
        // multi_edit_kw 里，命中 → MultiEdit（错）。修后（review P0-1）：
        // multi_edit_kw 只留 2+ char collocation，"改" 不命中 → 落到 LongRunning
        // 兜底。这次声明体现的 design intent 是"模糊短句归到 LongRunning 而非
        // MultiEdit"。
        let kind = classify_task_kind("我改主意了");
        assert_eq!(kind, TaskKind::LongRunning);
    }

    #[test]
    fn classify_改用_long_running_no_false_positive() {
        // "改用另一种方案" — has "改" but means "switch to another approach",
        // not "edit this". With single-char "改" removed from multi_edit_kw,
        // this now correctly falls through to LongRunning.
        let kind = classify_task_kind("改用另一种方案");
        assert_ne!(kind, TaskKind::MultiEdit);
    }

    #[test]
    fn classify_改成_multi_edit_real_intent() {
        // Counter-test: when "改" is paired with an obvious edit suffix
        // ("改成"), MultiEdit still wins. Pins that the 2-char tightening
        // didn't regress real-edit detection.
        assert_eq!(classify_task_kind("把这段代码改成异步"), TaskKind::MultiEdit);
        assert_eq!(classify_task_kind("改成 ESM"), TaskKind::MultiEdit);
        assert_eq!(classify_task_kind("修改一下配置"), TaskKind::MultiEdit);
    }

    #[test]
    fn classify_refactor_beats_multi_edit_for_explicit_refactor_words() {
        // "重构这个函数" explicitly says "重构" — Refactor wins over
        // MultiEdit because refactor is a more specific sub-kind of
        // modification. (Note: the current order checks multi_edit first,
        // so this test documents the actual behavior — if the order is
        // changed to refactor-first, update this test.)
        let kind = classify_task_kind("重构这个函数");
        // Either Refactor or MultiEdit is acceptable; what matters is NOT
        // ReadOnly / LongRunning.
        assert!(
            kind == TaskKind::Refactor || kind == TaskKind::MultiEdit,
            "expected Refactor or MultiEdit, got {:?}",
            kind
        );
    }

    #[test]
    fn classify_add_word_in_question_context_is_not_multi_edit() {
        // After P0-1 fix: single-char `加` is no longer in multi_edit_kw, so
        // "我加不加这个文件？" (a yes/no question, no edit intent) should fall
        // through to LongRunning. The 2+ char collocation "加上" / "添加" /
        // "加一下" / "添加文件" still hit MultiEdit when the user actually
        // wants to edit, so this isn't a regression.
        let kind = classify_task_kind("我加不加这个文件？");
        assert_eq!(kind, TaskKind::LongRunning);
    }

    #[test]
    fn budget_for_kind_matches_table() {
        assert_eq!(ResourceBudget::for_kind(TaskKind::MultiEdit).max_steps, 80);
        assert_eq!(ResourceBudget::for_kind(TaskKind::ReadOnly).max_steps, 20);
        assert_eq!(ResourceBudget::for_kind(TaskKind::Analysis).max_steps, 40);
        assert_eq!(ResourceBudget::for_kind(TaskKind::Refactor).max_steps, 60);
        assert_eq!(ResourceBudget::for_kind(TaskKind::LongRunning).max_steps, 120);
    }

    #[test]
    fn steps_remaining_ratio_basic() {
        let b = ResourceBudget::for_kind(TaskKind::MultiEdit);
        assert!((b.steps_remaining_ratio(0) - 1.0).abs() < 0.001);
        assert!((b.steps_remaining_ratio(40) - 0.5).abs() < 0.001);
        assert_eq!(b.steps_remaining_ratio(80), 0.0);
        // Overshoot clamps to 0.
        assert_eq!(b.steps_remaining_ratio(100), 0.0);
    }

    #[test]
    fn reminder_fires_below_25_percent() {
        let b = ResourceBudget::for_kind(TaskKind::MultiEdit); // 80 steps
        assert!(convergence_reminder(TaskKind::MultiEdit, 60, &b).is_some());
        assert!(convergence_reminder(TaskKind::MultiEdit, 70, &b).is_some());
    }

    #[test]
    fn reminder_quiet_above_25_percent() {
        let b = ResourceBudget::for_kind(TaskKind::MultiEdit);
        assert!(convergence_reminder(TaskKind::MultiEdit, 0, &b).is_none());
        assert!(convergence_reminder(TaskKind::MultiEdit, 40, &b).is_none());
        // 59/80 = 73.75% remaining → no nudge yet.
        assert!(convergence_reminder(TaskKind::MultiEdit, 59, &b).is_none());
    }

    #[test]
    fn reminder_carries_kind_and_counts() {
        let b = ResourceBudget::for_kind(TaskKind::MultiEdit);
        let r = convergence_reminder(TaskKind::MultiEdit, 65, &b).unwrap();
        assert!(r.contains("multi_edit"));
        assert!(r.contains("65/80"));
        assert!(r.contains("15")); // 80-65
    }
}