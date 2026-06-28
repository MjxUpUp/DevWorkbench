import { useState } from 'react';
import { useDashboardStore } from '../../stores/dashboardStore';
import {
  PRICING_PRESETS,
  estimateCost,
  type PriceTier,
  type TokenCounts,
} from './pricingPresets';

/** Editable per-tier price fields shown in the calculator. Order = display. */
const PRICE_FIELDS: { key: keyof PriceTier; label: string }[] = [
  { key: 'input', label: '输入单价' },
  { key: 'output', label: '输出单价' },
  { key: 'cacheRead', label: '缓存读单价' },
  { key: 'cacheWrite', label: '缓存写单价' },
];

/** Sentinel value for the "user edited prices off-script" pseudo-preset. */
const CUSTOM_KEY = 'custom';

/**
 * Number tier → per-field string map. The input is type="text" so it can hold
 * mid-typing states like '3.' or '' (a type="number" input can't — the browser
 * normalizes those away). estimateCost reads the derived number, the input shows
 * the raw string; the two stay in sync via onPresetChange/onPriceChange.
 */
const tierToText = (t: PriceTier): Record<keyof PriceTier, string> => ({
  input: String(t.input),
  output: String(t.output),
  cacheRead: String(t.cacheRead),
  cacheWrite: String(t.cacheWrite),
});

/**
 * B5 — BYOK transparent cost breakdown + a "what-if" unit-price calculator.
 *
 * The breakdown rows show the per-tier spend split (input / output / cache-read /
 * cache-write) the backend derives from recorded tokens × pricing.rs, so a user
 * bringing their own key sees *where* their money goes instead of one opaque
 * total. Token counts + USD both shown; each tier gets a proportional bar.
 *
 * The 单价换算 calculator re-estimates the SAME recorded tokens against any
 * model's list price — pick a preset (mirrors pricing.rs) or edit the per-tier
 * USD/1M fields. This is the escape hatch for providers whose pricing isn't in
 * pricing.rs (cost recorded as $0): pick the model, see what the tokens cost.
 * Purely client-side; nothing is persisted or sent to the backend.
 *
 * Renders when there is cost OR token data (so an UNKNOWN-priced provider — $0
 * cost but real tokens — still gets the calculator); hides cache rows the
 * provider didn't report (a $0 bar would falsely imply a feature).
 */
export function CostBreakdownCard() {
  const summary = useDashboardStore((s) => s.costSummary);
  // Calculator state: the editable tier + which preset is selected. Selecting a
  // preset copies its tier into the editable fields; editing any field flips the
  // selection to 'custom' (the user is now off-script).
  const [tier, setTier] = useState<PriceTier>(PRICING_PRESETS.claude_sonnet.tier);
  // Raw input strings (mid-typing states); tier holds the derived numbers.
  const [priceText, setPriceText] = useState<Record<keyof PriceTier, string>>(
    tierToText(PRICING_PRESETS.claude_sonnet.tier),
  );
  const [preset, setPreset] = useState<string>('claude_sonnet');

  // Render when there's cost OR token data. A provider priced as UNKNOWN has $0
  // cost but real tokens — the calculator is exactly for that case, so the card
  // isn't hidden just because cost is zero.
  if (!summary) return null;
  const hasTokens = summary.totalInputTokens + summary.totalOutputTokens > 0;
  if (summary.totalCost <= 0 && !hasTokens) return null;

  const tokens: TokenCounts = {
    input: summary.totalInputTokens ?? 0,
    output: summary.totalOutputTokens ?? 0,
    cacheRead: summary.totalCacheReadTokens ?? 0,
    cacheWrite: summary.totalCacheWriteTokens ?? 0,
  };

  const tiers = [
    {
      label: '输入',
      cost: summary.inputCost ?? 0,
      tokens: tokens.input,
      color: 'var(--cost-input)',
    },
    {
      label: '输出',
      cost: summary.outputCost ?? 0,
      tokens: tokens.output,
      color: 'var(--cost-output)',
    },
    {
      label: '缓存读取',
      cost: summary.cacheReadCost ?? 0,
      tokens: tokens.cacheRead,
      color: 'var(--cost-cache-read)',
      hideWhenZero: true,
    },
    {
      label: '缓存写入',
      cost: summary.cacheWriteCost ?? 0,
      tokens: tokens.cacheWrite,
      color: 'var(--cost-cache-write)',
      hideWhenZero: true,
    },
  ];

  // Cache tiers are provider-dependent (GLM reports none). Hide cache rows when
  // the provider never reported cache usage — a $0 bar would falsely imply a
  // feature that isn't there.
  const visibleTiers = tiers.filter((t) => !t.hideWhenZero || (t.tokens > 0 || t.cost > 0));
  const maxCost = Math.max(...visibleTiers.map((t) => t.cost), 0.0001);

  const onPresetChange = (key: string) => {
    setPreset(key);
    if (key !== CUSTOM_KEY) {
      const t = { ...PRICING_PRESETS[key].tier };
      setTier(t);
      setPriceText(tierToText(t));
    }
  };
  const onPriceChange = (field: keyof PriceTier, text: string) => {
    // Keep the raw string (so '3.' / '' survive mid-typing) and derive the number.
    setPriceText((prev) => ({ ...prev, [field]: text }));
    setTier((t) => ({ ...t, [field]: parseFloat(text) || 0 }));
    setPreset(CUSTOM_KEY);
  };

  const estimatedCost = estimateCost(tokens, tier);

  return (
    <div className="dashboard-chart-panel cost-breakdown">
      <div className="cost-breakdown-header">
        <span className="cost-breakdown-title">费用构成</span>
        <span className="cost-breakdown-total">${summary.totalCost.toFixed(4)}</span>
      </div>

      <div className="cost-breakdown-rows">
        {visibleTiers.map((t) => (
          <div key={t.label} className="cost-breakdown-row">
            <div className="cost-breakdown-row-head">
              <span className="cost-breakdown-tier-label">
                <span className="cost-breakdown-dot" style={{ backgroundColor: t.color }} />
                {t.label}
              </span>
              <span className="cost-breakdown-tier-cost">${t.cost.toFixed(4)}</span>
            </div>
            <div className="cost-breakdown-bar-track">
              <div
                className="cost-breakdown-bar-fill"
                style={{ width: `${(t.cost / maxCost) * 100}%`, backgroundColor: t.color }}
              />
            </div>
            <div className="cost-breakdown-tokens">{t.tokens.toLocaleString()} tokens</div>
          </div>
        ))}
      </div>

      <div className="cost-calculator">
        <div className="cost-calculator-head">
          <span className="cost-calculator-title">单价换算</span>
          <select
            className="cost-calculator-select"
            value={preset}
            onChange={(e) => onPresetChange(e.target.value)}
            aria-label="模型单价预设"
          >
            {Object.entries(PRICING_PRESETS).map(([k, p]) => (
              <option key={k} value={k}>
                {p.label}
              </option>
            ))}
            <option value="custom">自定义</option>
          </select>
        </div>
        <div className="cost-calculator-prices">
          {PRICE_FIELDS.map(({ key, label }) => (
            <label key={key} className="cost-calculator-price">
              <span className="cost-calculator-price-label">{label}</span>
              <span className="cost-calculator-price-input">
                <input
                  type="text"
                  inputMode="decimal"
                  value={priceText[key]}
                  onChange={(e) => onPriceChange(key, e.target.value)}
                  aria-label={`${label}（美元/百万 tokens）`}
                />
                <span className="cost-calculator-price-unit">$/M</span>
              </span>
            </label>
          ))}
        </div>
        <div className="cost-calculator-result">
          <span className="cost-calculator-result-label">按此单价估算</span>
          <span className="cost-calculator-estimate">≈ ${estimatedCost.toFixed(4)}</span>
          {summary.totalCost > 0 && (
            <span className="cost-calculator-record">记录 ${summary.totalCost.toFixed(4)}</span>
          )}
        </div>
      </div>

      <div className="cost-breakdown-note">
        BYOK 透明成本 · 按各厂商公开定价估算（非实际账单）
      </div>
    </div>
  );
}
