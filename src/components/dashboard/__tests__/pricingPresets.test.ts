import { describe, it, expect } from 'vitest';
import { PRICING_PRESETS, estimateCost, type TokenCounts } from '../pricingPresets';

/**
 * The calculator's pricing presets + estimate formula. estimateCost must agree
 * with how the backend derives cost (pricing.rs `cost_breakdown`), so the
 * calculator's "≈ estimate" matches the recorded cost when the same prices are
 * used — that's the whole point of the comparison line.
 */
describe('pricingPresets', () => {
  describe('estimateCost', () => {
    it('sums each tier: tokens × price-per-million', () => {
      // 1M of each tier at Claude Sonnet list = $3 + $15 + $0.3 + $3.75 = $22.05.
      const tokens: TokenCounts = {
        input: 1_000_000,
        output: 1_000_000,
        cacheRead: 1_000_000,
        cacheWrite: 1_000_000,
      };
      const cost = estimateCost(tokens, PRICING_PRESETS.claude_sonnet.tier);
      expect(Math.abs(cost - 22.05)).toBeLessThan(1e-9);
    });

    it('zero tokens cost nothing, even at Opus prices', () => {
      const zero: TokenCounts = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };
      expect(estimateCost(zero, PRICING_PRESETS.claude_opus.tier)).toBe(0);
    });

    it('mirrors the backend per-token formula', () => {
      // pricing.rs: tokens/1M × price. 500k input @ DeepSeek $0.27 = $0.135.
      const tokens: TokenCounts = { input: 500_000, output: 0, cacheRead: 0, cacheWrite: 0 };
      const cost = estimateCost(tokens, PRICING_PRESETS.deepseek.tier);
      expect(Math.abs(cost - 0.135)).toBeLessThan(1e-9);
    });

    it('zero-price tiers (GLM cache) contribute nothing', () => {
      // GLM has cacheRead=0 / cacheWrite=0 → cache tokens cost $0 (honest absence,
      // matching pricing.rs where unpriced tiers are 0, not fabricated).
      const tokens: TokenCounts = {
        input: 0,
        output: 0,
        cacheRead: 1_000_000,
        cacheWrite: 1_000_000,
      };
      expect(estimateCost(tokens, PRICING_PRESETS.zhipu_glm.tier)).toBe(0);
    });
  });

  describe('PRICING_PRESETS', () => {
    it('every preset has non-negative prices + a label', () => {
      for (const [key, p] of Object.entries(PRICING_PRESETS)) {
        expect(key.length).toBeGreaterThan(0);
        expect(p.label.length).toBeGreaterThan(0);
        const { input, output, cacheRead, cacheWrite } = p.tier;
        expect(input).toBeGreaterThanOrEqual(0);
        expect(output).toBeGreaterThanOrEqual(0);
        expect(cacheRead).toBeGreaterThanOrEqual(0);
        expect(cacheWrite).toBeGreaterThanOrEqual(0);
      }
    });

    it('includes the core families mirrored from pricing.rs', () => {
      // Keys mirror ModelPricing::* in src-tauri/src/cost/pricing.rs.
      for (const key of [
        'zhipu_glm',
        'claude_opus',
        'claude_sonnet',
        'claude_haiku',
        'gpt_41',
        'gpt_4o',
        'deepseek',
      ]) {
        expect(PRICING_PRESETS[key]).toBeDefined();
      }
    });
  });
});
