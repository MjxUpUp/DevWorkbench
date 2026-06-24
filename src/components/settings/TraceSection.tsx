import { useEffect, useState } from 'react';
import { Button } from '../ui/Button/Button';
import { useTraceSettingsStore } from '../../stores/traceSettingsStore';

/**
 * "LLM 追踪" settings section — configures trace retention. Mirrors the
 * 2026-06-19 trace observability research: default infinite (Phoenix's
 * infinite-by-default semantics), user-tunable 3–365 days, plus a manual
 * "clean up now" that prunes expired traces and VACUUMs to reclaim disk. The
 * traces themselves are read by TraceView (traceStore); this section only
 * manages how long they live.
 */
export function TraceSection() {
  const settings = useTraceSettingsStore((s) => s.settings);
  const loading = useTraceSettingsStore((s) => s.loading);
  const error = useTraceSettingsStore((s) => s.error);
  const lastPruned = useTraceSettingsStore((s) => s.lastPruned);
  const fetchSettings = useTraceSettingsStore((s) => s.fetchSettings);
  const setRetention = useTraceSettingsStore((s) => s.setRetention);
  const pruneNow = useTraceSettingsStore((s) => s.pruneNow);
  const [draft, setDraft] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    fetchSettings();
  }, [fetchSettings]);

  const currentDays = settings?.retention_days ?? null;

  const apply = async () => {
    const n = draft === '' ? null : Number(draft);
    setBusy(true);
    // null / 0 / non-positive → infinite (matches the Rust prune_old_traces gate).
    await setRetention(n && n > 0 ? n : null);
    setDraft('');
    setBusy(false);
  };

  const cleanup = async () => {
    setBusy(true);
    await pruneNow();
    setBusy(false);
  };

  return (
    <div className="settings-section trace-section">
      <div className="settings-section-header">
        <h3>LLM 调用追踪</h3>
        <p className="settings-section-desc">
          记录每次 LLM HTTP 调用的请求与响应，用于排查秒败、prompt 回归、计费核对。按保留期自动清理过期记录。
        </p>
      </div>

      {loading && <p className="settings-section-hint">加载中…</p>}
      {error && <p className="settings-section-error">出错：{error}</p>}

      <div className="settings-field-row">
        <label htmlFor="trace-retention">保留天数</label>
        <input
          id="trace-retention"
          type="number"
          min={0}
          max={365}
          placeholder={currentDays === null ? '无限（留空）' : String(currentDays)}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          disabled={busy}
        />
        <Button variant="primary" size="sm" onClick={apply} disabled={busy}>
          应用
        </Button>
        <span className="settings-section-hint">
          空 / 0 = 无限保留；3–365 = 自动清理该天数之前的记录（启动时执行）。
        </span>
      </div>

      <div className="settings-field-row">
        <Button variant="secondary" size="sm" onClick={cleanup} disabled={busy}>
          立即清理过期记录
        </Button>
        <span className="settings-section-hint">
          {lastPruned !== null
            ? `上次清理：删除 ${lastPruned} 条`
            : currentDays === null
              ? '当前无限保留，无过期记录'
              : `将清理 ${currentDays} 天前的记录并压缩数据库`}
        </span>
      </div>

      <p className="settings-section-hint">上次 VACUUM：{settings?.last_vacuum_at ?? '从未'}</p>

      <div className="mission-note" style={{ marginTop: 'var(--space-4)' }}>
        <strong>💡 查看调用详情</strong><br />
        保留天数只控制存储清理。实时查看每次 LLM 调用的请求体/响应体/延迟/状态，
        请在主界面左侧导航点「Trace」切换到 LLM 追踪视图。
      </div>
    </div>
  );
}
