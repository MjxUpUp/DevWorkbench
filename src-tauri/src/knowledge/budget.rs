//! Token-budgeted selection for knowledge injected into the system prompt (D6).
//!
//! The experience / memory prompt suffixes used to take a HARDCODED number of
//! entries (`take(3)` failures, `take(5)` memories). That's the wrong unit:
//! prompt cost is measured in TOKENS, not rows. A few verbose entries could
//! blow past the budget while many terse ones were artificially capped. These
//! helpers pick entries front-to-back (after the caller's own ranking) while
//! their RENDERED form fits a token budget — so we budget real prompt cost.
//!
//! The estimate is deliberately coarse (~3 chars/token) and conservative.
//! Exact tokenization is model-specific and not worth a dependency here;
//! over-counting only leaves a little unused budget, which is safe.

/// Rough token estimate for prompt-budgeting. ~3 chars/token covers mixed CJK +
/// ASCII (the common case for this project's Chinese prompts) and rounds up so
/// a single char still counts as one token (never zero for non-empty input).
pub fn estimate_tokens(s: &str) -> usize {
    let chars = s.chars().count();
    if chars == 0 {
        0
    } else {
        (chars + 2) / 3
    }
}

/// Pick items front-to-back while their rendered form fits within a token
/// budget. `render` produces the EXACT line that will go in the prompt, so the
/// budget reflects real cost (title + content), not raw storage. An item that
/// individually exceeds the remaining budget is SKIPPED (not force-included),
/// so one oversized entry can't consume the whole budget while a later small
/// one would have fit.
pub fn select_within_budget<T, S, F>(items: &[T], budget_tokens: usize, render: F) -> Vec<&T>
where
    F: Fn(&T) -> S,
    S: AsRef<str>,
{
    let mut used = 0usize;
    items
        .iter()
        .filter(|item| {
            let cost = estimate_tokens(render(item).as_ref());
            if cost == 0 {
                return false; // an empty rendered line adds no signal, only padding
            }
            if used + cost <= budget_tokens {
                used += cost;
                true
            } else {
                false
            }
        })
        .collect()
}

/// Token budgets for the two prompt suffixes. Conservative: the experience
/// suffix is terse failure-warning titles; the memory suffix includes up to 200
/// chars of content per entry. Together they stay well under ~2k tokens so the
/// system prompt's knowledge tail never crowds out the actual task.
pub const EXPERIENCE_BUDGET_TOKENS: usize = 600;
pub const MEMORY_BUDGET_TOKENS: usize = 1000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_tokens_rounds_up_and_handles_empty() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("ab"), 1); // (2+2)/3 = 1
        assert_eq!(estimate_tokens("abc"), 1); // (3+2)/3 = 1
        assert_eq!(estimate_tokens("abcd"), 2); // (4+2)/3 = 2
        assert_eq!(estimate_tokens("abcdef"), 2); // exactly 2, no leftover
    }

    #[test]
    fn select_picks_front_to_back_until_budget_exhausted() {
        // Each rendered line is 5 chars → 2 tokens. Budget 5 → first two fit
        // (2+2=4 ≤ 5); the third would push it to 6 > 5 → skipped.
        let items = vec!["aaaaa".to_string(), "bbbbb".to_string(), "ccccc".to_string()];
        let picked = select_within_budget(&items, 5, |s: &String| s.clone());
        assert_eq!(picked.len(), 2);
        assert_eq!(*picked[0], "aaaaa");
        assert_eq!(*picked[1], "bbbbb");
    }

    #[test]
    fn select_skips_oversized_entry_keeps_later_small_one() {
        // First entry alone exceeds budget → skipped; the small second entry
        // still fits. Force-including the oversized one would waste the budget.
        let items = vec!["x".repeat(30), "ab".to_string()];
        let picked = select_within_budget(&items, 3, |s: &String| s.clone());
        assert_eq!(picked.len(), 1);
        assert_eq!(*picked[0], "ab");
    }

    #[test]
    fn select_empty_render_line_is_skipped() {
        // A blank rendered line carries no signal and would only pad the prompt.
        let items = vec!["".to_string(), "real".to_string()];
        let picked = select_within_budget(&items, 100, |s: &String| s.clone());
        assert_eq!(picked.len(), 1);
        assert_eq!(*picked[0], "real");
    }

    #[test]
    fn select_zero_budget_returns_empty() {
        let items = vec!["a".to_string()];
        assert!(select_within_budget(&items, 0, |s: &String| s.clone()).is_empty());
    }
}
