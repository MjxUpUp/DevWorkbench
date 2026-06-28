/**
 * Coarse per-family token pricing (USD per 1M tokens), mirroring the Rust
 * `ModelPricing` constants in `src-tauri/src/cost/pricing.rs`. The cost-breakdown
 * card's calculator pre-fills these as a convenience so a user can re-estimate
 * their spend against any model's list price; the user can override any field
 * (or pick "自定义") — these are reference prices for estimation, not
 * authoritative billing.
 *
 * Kept as a front-end copy rather than an IPC call because the values are
 * coarse reference data (the backend already prices recorded tokens via
 * pricing.rs); the calculator is a client-side "what-if" tool. If you change a
 * price here, update `pricing.rs` too (and vice versa).
 */

/** USD per 1M tokens for each spend tier. */
export interface PriceTier {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

/** A named model-family preset (label + its price tier). */
export interface PricingPreset {
  label: string;
  tier: PriceTier;
}

/**
 * Preset key → preset. Values match `ModelPricing::*` in pricing.rs exactly
 * (ZHIPU_GLM / CLAUDE_OPUS / CLAUDE_SONNET / CLAUDE_HAIKU / GPT_41 / GPT_4O /
 * DEEPSEEK). UNKNOWN is omitted — that's the case the calculator exists for:
 * the user types the price themselves.
 */
export const PRICING_PRESETS: Record<string, PricingPreset> = {
  zhipu_glm: {
    label: '智谱 GLM-4',
    tier: { input: 1.0, output: 3.2, cacheRead: 0, cacheWrite: 0 },
  },
  claude_opus: {
    label: 'Claude Opus',
    tier: { input: 15.0, output: 75.0, cacheRead: 1.5, cacheWrite: 18.75 },
  },
  claude_sonnet: {
    label: 'Claude Sonnet',
    tier: { input: 3.0, output: 15.0, cacheRead: 0.3, cacheWrite: 3.75 },
  },
  claude_haiku: {
    label: 'Claude Haiku',
    tier: { input: 0.8, output: 4.0, cacheRead: 0.08, cacheWrite: 1.0 },
  },
  gpt_41: {
    label: 'GPT-4.1',
    tier: { input: 2.0, output: 8.0, cacheRead: 0.5, cacheWrite: 0 },
  },
  gpt_4o: {
    label: 'GPT-4o',
    tier: { input: 2.5, output: 10.0, cacheRead: 1.25, cacheWrite: 0 },
  },
  deepseek: {
    label: 'DeepSeek',
    tier: { input: 0.27, output: 1.1, cacheRead: 0.07, cacheWrite: 0 },
  },
};

/** Per-tier token counts (the calculator's input). */
export interface TokenCounts {
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
}

/**
 * Estimate USD cost = Σ(tier_tokens × price_per_million) / 1_000_000.
 * Mirrors `cost_breakdown` in pricing.rs so the calculator's number agrees with
 * how the backend derives the recorded cost when the same prices are used.
 */
export function estimateCost(tokens: TokenCounts, tier: PriceTier): number {
  const per = (count: number, price: number) => (count / 1_000_000) * price;
  return (
    per(tokens.input, tier.input) +
    per(tokens.output, tier.output) +
    per(tokens.cacheRead, tier.cacheRead) +
    per(tokens.cacheWrite, tier.cacheWrite)
  );
}
