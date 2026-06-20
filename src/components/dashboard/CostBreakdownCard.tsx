import { useDashboardStore } from '../../stores/dashboardStore';

/**
 * B5 — BYOK transparent cost breakdown. Renders the per-tier spend split
 * (input / output / cache-read / cache-write) the backend now derives, so a
 * user bringing their own key can see *where* their money goes instead of one
 * opaque total. Token counts + USD both shown; each tier gets a proportional
 * bar against the largest tier so relative weight is visible at a glance.
 *
 * Reads `costSummary` from the store (the raw CostSummary, not the collapsed
 * DashboardStats). Renders nothing when there's no cost data yet (fresh app /
 * no model calls) and hides the cache rows when the provider didn't report any
 * (GLM folds cache into input — honest absence, not a $0 bar that implies a
 * feature).
 */
export function CostBreakdownCard() {
  const summary = useDashboardStore((s) => s.costSummary);

  // No data yet (fresh app / zero model calls) → don't render an empty card.
  if (!summary || summary.totalCost <= 0) return null;

  const tiers = [
    {
      label: '输入',
      cost: summary.inputCost ?? 0,
      tokens: summary.totalInputTokens ?? 0,
      color: 'var(--cost-input)',
    },
    {
      label: '输出',
      cost: summary.outputCost ?? 0,
      tokens: summary.totalOutputTokens ?? 0,
      color: 'var(--cost-output)',
    },
    {
      label: '缓存读取',
      cost: summary.cacheReadCost ?? 0,
      tokens: summary.totalCacheReadTokens ?? 0,
      color: 'var(--cost-cache-read)',
      hideWhenZero: true,
    },
    {
      label: '缓存写入',
      cost: summary.cacheWriteCost ?? 0,
      tokens: summary.totalCacheWriteTokens ?? 0,
      color: 'var(--cost-cache-write)',
      hideWhenZero: true,
    },
  ];

  // Cache tiers are provider-dependent (GLM reports none). Hide cache rows when
  // the provider never reported cache usage — a $0 bar would falsely imply a
  // feature that isn't there.
  const visibleTiers = tiers.filter((t) => !t.hideWhenZero || (t.tokens > 0 || t.cost > 0));
  const maxCost = Math.max(...visibleTiers.map((t) => t.cost), 0.0001);

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
            <div className="cost-breakdown-tokens">
              {t.tokens.toLocaleString()} tokens
            </div>
          </div>
        ))}
      </div>

      <div className="cost-breakdown-note">
        BYOK 透明成本 · 按各厂商公开定价估算（非实际账单）
      </div>
    </div>
  );
}
