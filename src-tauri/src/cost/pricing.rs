//! Model pricing + cost calculation. Ported from AgentFare's
//! `packages/models/src/types.ts` (`ModelPricing`) and
//! `packages/core/src/tracker/cost-tracker.ts` (`calculateCostFromEntry`), then
//! extended (B5) with per-provider list pricing + Anthropic-style prompt-cache
//! economics so the cost dashboard can show a transparent input/output/cache
//! breakdown instead of a single opaque dollar figure. Prices are USD per
//! million tokens.
//!
//! Sources for the per-family constants are the providers' public list prices
//! (Anthropic / OpenAI / Zhipu). They're deliberately coarse (one tier per
//! family) — the point is honest order-of-magnitude transparency under BYOK,
//! not cent-accurate billing. A model id we don't recognize falls back to
//! `UNKNOWN` (zero), so cost is honestly absent rather than fabricated.

/// Per-model token pricing (USD per 1M tokens), including prompt-cache tiers.
/// Cache rates follow Anthropic's convention: a cache read is ~10% of the input
/// price, a cache write (creation) is ~125% — the economics that make
/// prompt-caching worth doing. Models without a published cache price leave
/// these at 0 (cache tokens then contribute $0, which is honest).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
    pub cache_read_per_million: f64,
    pub cache_write_per_million: f64,
}

impl ModelPricing {
    /// Zhipu GLM-4 family — the value AgentFare calibrated for the GLM family
    /// on the Anthropic-compatible endpoint. GLM has no public prompt-cache
    /// pricing as of 2026-06, so cache tiers are 0. Renamed from `GLM` in the
    /// multi-provider refactor to name the vendor (Zhipu) honestly, since a bare
    /// `GLM` constant reads as if it priced any "glm-*" string rather than one
    /// vendor's family.
    pub const ZHIPU_GLM: Self = Self {
        input_per_million: 1.0,
        output_per_million: 3.2,
        cache_read_per_million: 0.0,
        cache_write_per_million: 0.0,
    };

    /// Claude Sonnet 4.x — Anthropic list price ($3/$15) with the standard
    /// 90%-read / 25%-write-premium cache tiers.
    pub const CLAUDE_SONNET: Self = Self {
        input_per_million: 3.0,
        output_per_million: 15.0,
        cache_read_per_million: 0.3,
        cache_write_per_million: 3.75,
    };

    /// Claude Opus 4.x — $15/$75 list, same cache ratio.
    pub const CLAUDE_OPUS: Self = Self {
        input_per_million: 15.0,
        output_per_million: 75.0,
        cache_read_per_million: 1.5,
        cache_write_per_million: 18.75,
    };

    /// Claude Haiku — $0.80/$4 list.
    pub const CLAUDE_HAIKU: Self = Self {
        input_per_million: 0.8,
        output_per_million: 4.0,
        cache_read_per_million: 0.08,
        cache_write_per_million: 1.0,
    };

    /// OpenAI GPT-4o — $2.50/$10 list (no separate cache tier; OpenAI folds
    /// cached input into the discounted input rate, kept at 0 here).
    pub const GPT_4O: Self = Self {
        input_per_million: 2.5,
        output_per_million: 10.0,
        cache_read_per_million: 1.25,
        cache_write_per_million: 0.0,
    };

    /// DeepSeek V3/R1 — list price $0.27/$1.10 with a steeply-discounted cache-
    /// read tier ($0.07, ~75% off). Added in the multi-provider refactor so the
    /// OpenAI-protocol DeepSeek preset prices honestly. No cache-write tier.
    pub const DEEPSEEK: Self = Self {
        input_per_million: 0.27,
        output_per_million: 1.10,
        cache_read_per_million: 0.07,
        cache_write_per_million: 0.0,
    };

    /// OpenAI GPT-4.1 — $2/$8 list, cached input at $0.5/M (OpenAI's 75%-off
    /// cache-read tier). Distinct from GPT-4o (different price + a real cache
    /// tier), so matched before `gpt-4o` in `pricing_for`.
    pub const GPT_41: Self = Self {
        input_per_million: 2.0,
        output_per_million: 8.0,
        cache_read_per_million: 0.5,
        cache_write_per_million: 0.0,
    };

    /// Unknown model — zero pricing. The caller can still record token counts;
    /// cost is honestly absent rather than fabricated.
    pub const UNKNOWN: Self = Self {
        input_per_million: 0.0,
        output_per_million: 0.0,
        cache_read_per_million: 0.0,
        cache_write_per_million: 0.0,
    };
}

/// Resolve pricing for a model id by family. Matching is case-insensitive on
/// substrings so version suffixes (`claude-sonnet-4-5-20250929`) still resolve.
/// Order matters: `opus` is checked before the generic `claude` fallback so a
/// haiku/sonnet id never accidentally lands on the opus tier.
pub fn pricing_for(model: &str) -> ModelPricing {
    let m = model.to_ascii_lowercase();
    if m.contains("glm") {
        ModelPricing::ZHIPU_GLM
    } else if m.contains("deepseek") {
        ModelPricing::DEEPSEEK
    } else if m.contains("opus") {
        ModelPricing::CLAUDE_OPUS
    } else if m.contains("haiku") {
        ModelPricing::CLAUDE_HAIKU
    } else if m.contains("sonnet") || m.contains("claude") {
        ModelPricing::CLAUDE_SONNET
    } else if m.contains("gpt-4.1") {
        ModelPricing::GPT_41
    } else if m.contains("gpt-4o") {
        ModelPricing::GPT_4O
    } else {
        ModelPricing::UNKNOWN
    }
}

/// One model request's token usage, broken out by tier. `cache_read` +
/// `cache_write` are the Anthropic prompt-cache token counts
/// (`cache_read_input_tokens` / `cache_creation_input_tokens`); providers that
/// don't report them leave them at 0.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TokenUsage {
    pub input: u32,
    pub output: u32,
    pub cache_read: u32,
    pub cache_write: u32,
}

impl TokenUsage {
    pub fn new(input: u32, output: u32) -> Self {
        Self { input, output, cache_read: 0, cache_write: 0 }
    }

    /// Saturating add — used to fold a stream's per-event usage deltas into one
    /// turn total without overflowing on a very long stream.
    pub fn saturating_add(self, other: Self) -> Self {
        Self {
            input: self.input.saturating_add(other.input),
            output: self.output.saturating_add(other.output),
            cache_read: self.cache_read.saturating_add(other.cache_read),
            cache_write: self.cache_write.saturating_add(other.cache_write),
        }
    }
}

/// A transparent per-call cost split — the B5 moat. Each component is reported
/// independently so the dashboard can show "input $X · output $Y · cache $Z"
/// instead of one number, and a user can see exactly where their spend goes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct CostBreakdown {
    pub input_cost: f64,
    pub output_cost: f64,
    pub cache_read_cost: f64,
    pub cache_write_cost: f64,
}

impl CostBreakdown {
    pub fn total(&self) -> f64 {
        self.input_cost + self.output_cost + self.cache_read_cost + self.cache_write_cost
    }
}

/// Compute the full cost breakdown for a usage against a pricing tier. The
/// legacy scalar [`cost`] is now `cost_breakdown(...).total()` for back-compat.
pub fn cost_breakdown(usage: TokenUsage, pricing: ModelPricing) -> CostBreakdown {
    let per = |tokens: u32, price: f64| (tokens as f64 / 1_000_000.0) * price;
    CostBreakdown {
        input_cost: per(usage.input, pricing.input_per_million),
        output_cost: per(usage.output, pricing.output_per_million),
        cache_read_cost: per(usage.cache_read, pricing.cache_read_per_million),
        cache_write_cost: per(usage.cache_write, pricing.cache_write_per_million),
    }
}

/// Compute USD cost (total) for a token usage against a pricing tier. Mirrors
/// AgentFare `calculateCostFromEntry`; kept as the scalar entry point for
/// callers that don't need the split. The breakdown variant is preferred.
pub fn cost(input_tokens: u32, output_tokens: u32, pricing: ModelPricing) -> f64 {
    cost_breakdown(TokenUsage::new(input_tokens, output_tokens), pricing).total()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_formula_matches_pricing() {
        // 2M input @ $1 + 1M output @ $3.2 = $2 + $3.2 = $5.2.
        let c = cost(2_000_000, 1_000_000, ModelPricing::ZHIPU_GLM);
        assert!((c - 5.2).abs() < 1e-9, "{c}");
    }

    #[test]
    fn pricing_for_resolves_by_family() {
        assert_eq!(pricing_for("glm-4.6"), ModelPricing::ZHIPU_GLM);
        assert_eq!(pricing_for("GLM-4.5"), ModelPricing::ZHIPU_GLM);
        assert_eq!(pricing_for("claude-sonnet-4-5-20250929"), ModelPricing::CLAUDE_SONNET);
        assert_eq!(pricing_for("claude-opus-4-1"), ModelPricing::CLAUDE_OPUS);
        assert_eq!(pricing_for("claude-haiku-4-5"), ModelPricing::CLAUDE_HAIKU);
        assert_eq!(pricing_for("gpt-4o-2024-11-20"), ModelPricing::GPT_4O);
        // opus checked before the generic claude fallback (order matters).
        assert_ne!(pricing_for("claude-opus-x"), ModelPricing::CLAUDE_SONNET);
        assert_eq!(pricing_for("deepseek-chat"), ModelPricing::DEEPSEEK);
        assert_eq!(pricing_for("deepseek-v3"), ModelPricing::DEEPSEEK);
        assert_eq!(pricing_for("gpt-4.1-2025-04-14"), ModelPricing::GPT_41);
        assert_eq!(pricing_for(""), ModelPricing::UNKNOWN);
    }

    #[test]
    fn unknown_pricing_yields_zero_cost() {
        assert_eq!(cost(999_999, 999_999, ModelPricing::UNKNOWN), 0.0);
    }

    #[test]
    fn zero_tokens_yield_zero_cost() {
        assert_eq!(cost(0, 0, ModelPricing::ZHIPU_GLM), 0.0);
    }

    #[test]
    fn breakdown_splits_input_output_and_cache() {
        // Sonnet: 1M input @ $3, 1M output @ $15, 1M cache-read @ $0.30,
        // 1M cache-write @ $3.75 → total $3 + $15 + $0.30 + $3.75 = $22.05.
        let usage = TokenUsage {
            input: 1_000_000,
            output: 1_000_000,
            cache_read: 1_000_000,
            cache_write: 1_000_000,
        };
        let b = cost_breakdown(usage, ModelPricing::CLAUDE_SONNET);
        assert!((b.input_cost - 3.0).abs() < 1e-9, "input: {}", b.input_cost);
        assert!((b.output_cost - 15.0).abs() < 1e-9, "output: {}", b.output_cost);
        assert!((b.cache_read_cost - 0.3).abs() < 1e-9, "cache_read: {}", b.cache_read_cost);
        assert!((b.cache_write_cost - 3.75).abs() < 1e-9, "cache_write: {}", b.cache_write_cost);
        assert!((b.total() - 22.05).abs() < 1e-9, "total: {}", b.total());
    }

    #[test]
    fn breakdown_cache_zero_when_unreported() {
        // GLM has no cache pricing → cache tokens contribute $0 even if a
        // caller passes them (honest: we can't price what the provider doesn't).
        let usage = TokenUsage {
            input: 1_000_000,
            output: 0,
            cache_read: 500_000,
            cache_write: 500_000,
        };
        let b = cost_breakdown(usage, ModelPricing::ZHIPU_GLM);
        assert_eq!(b.cache_read_cost, 0.0);
        assert_eq!(b.cache_write_cost, 0.0);
        assert!((b.total() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn token_usage_saturating_add_folds_stream_deltas() {
        let a = TokenUsage { input: 10, output: 5, cache_read: 2, cache_write: 1 };
        let b = TokenUsage { input: 20, output: 7, cache_read: 3, cache_write: 4 };
        let sum = a.saturating_add(b);
        assert_eq!(sum, TokenUsage { input: 30, output: 12, cache_read: 5, cache_write: 5 });
    }

    #[test]
    fn cost_equals_breakdown_total() {
        // The scalar entry point must agree with the breakdown total.
        let usage = TokenUsage::new(1_500_000, 750_000);
        let direct = cost(usage.input, usage.output, ModelPricing::CLAUDE_SONNET);
        let via_breakdown = cost_breakdown(usage, ModelPricing::CLAUDE_SONNET).total();
        assert!((direct - via_breakdown).abs() < 1e-12);
    }
}
