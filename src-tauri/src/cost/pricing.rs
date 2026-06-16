//! Model pricing + cost calculation. Ported from AgentFare's
//! `packages/models/src/types.ts` (`ModelPricing`) and
//! `packages/core/src/tracker/cost-tracker.ts` (`calculateCostFromEntry`).
//! Prices are USD per million tokens.
//!
//! P0 uses a fixed default calibrated against AgentFare's GLM entry; real
//! per-model pricing should be sourced from `providers.toml` (P1) so the table
//! tracks list-price changes without a code edit.

/// Per-model token pricing (USD per 1M tokens).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    pub input_per_million: f64,
    pub output_per_million: f64,
}

impl ModelPricing {
    /// Default GLM pricing — the value AgentFare calibrated for the GLM family
    /// on the Anthropic-compatible endpoint. Used for any GLM model id in P0.
    pub const GLM: Self = Self {
        input_per_million: 1.0,
        output_per_million: 3.2,
    };

    /// Unknown model — zero pricing. The caller can still record token counts;
    /// cost is honestly absent rather than fabricated.
    pub const UNKNOWN: Self = Self {
        input_per_million: 0.0,
        output_per_million: 0.0,
    };
}

/// Resolve pricing for a model id. Any GLM-family model (case-insensitive)
/// resolves to the GLM default; everything else is `UNKNOWN` (P1: read real
/// per-model prices from providers config).
pub fn pricing_for(model: &str) -> ModelPricing {
    if model.to_ascii_lowercase().contains("glm") {
        ModelPricing::GLM
    } else {
        ModelPricing::UNKNOWN
    }
}

/// Compute USD cost for a token usage against a pricing tier. Mirrors AgentFare
/// `calculateCostFromEntry`: `(input / 1e6) * input_price + (output / 1e6) *
/// output_price`.
pub fn cost(input_tokens: u32, output_tokens: u32, pricing: ModelPricing) -> f64 {
    (input_tokens as f64 / 1_000_000.0) * pricing.input_per_million
        + (output_tokens as f64 / 1_000_000.0) * pricing.output_per_million
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cost_formula_matches_pricing() {
        // 2M input @ $1 + 1M output @ $3.2 = $2 + $3.2 = $5.2.
        let c = cost(2_000_000, 1_000_000, ModelPricing::GLM);
        assert!((c - 5.2).abs() < 1e-9, "{c}");
    }

    #[test]
    fn pricing_for_resolves_glm_family_case_insensitive() {
        assert_eq!(pricing_for("glm-4.6"), ModelPricing::GLM);
        assert_eq!(pricing_for("GLM-4.5"), ModelPricing::GLM);
        assert_eq!(pricing_for("glm"), ModelPricing::GLM);
        assert_eq!(pricing_for("claude-sonnet"), ModelPricing::UNKNOWN);
        assert_eq!(pricing_for(""), ModelPricing::UNKNOWN);
    }

    #[test]
    fn unknown_pricing_yields_zero_cost() {
        assert_eq!(cost(999_999, 999_999, ModelPricing::UNKNOWN), 0.0);
    }

    #[test]
    fn zero_tokens_yield_zero_cost() {
        assert_eq!(cost(0, 0, ModelPricing::GLM), 0.0);
    }
}
