import { useEffect, useState } from 'react';
import { Line } from 'react-chartjs-2';
import { evalApi, type TrendPoint, type EvalRunRow, type Grade } from '../../utils/evalApi';

/**
 * B7 trajectory-eval panel — the user-visible half of the eval pipeline. Shows
 * the daily avg-score regression curve (`eval_trend`) and the most recent
 * scored runs (`list_runs`). Mirrors the OpenAI Agents SDK trajectory-evaluation
 * rubric: optimal / suboptimal / incorrect. Sits next to `QualityHistory` in
 * the Usage Stats section (forge gate history is a different, orthogonal
 * quality dimension). The `run_session` trigger — picking a session + reference
 * — is deferred; this view is read-only trend + history for now.
 */
const GRADE_LABEL: Record<Grade, { text: string; cls: string }> = {
  optimal: { text: '最优', cls: 'eval-grade-optimal' },
  suboptimal: { text: '次优', cls: 'eval-grade-suboptimal' },
  incorrect: { text: '错误', cls: 'eval-grade-incorrect' },
};

export function EvalPanel() {
  const [trend, setTrend] = useState<TrendPoint[]>([]);
  const [runs, setRuns] = useState<EvalRunRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      setLoading(true);
      try {
        const [t, r] = await Promise.all([evalApi.trend(30), evalApi.listRuns(undefined, 20)]);
        if (cancelled) return;
        setTrend(t);
        setRuns(r);
        setError(null);
      } catch (e) {
        if (!cancelled) setError(String(e));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const data = {
    labels: trend.map((p) => p.date),
    datasets: [
      {
        label: '平均得分',
        data: trend.map((p) => p.avg_score),
        fill: true,
        borderColor: 'var(--accent)',
        backgroundColor: 'rgba(37, 99, 235, 0.08)',
        tension: 0.3,
        pointRadius: 3,
      },
    ],
  };
  const options = {
    responsive: true,
    maintainAspectRatio: false,
    plugins: {
      legend: { display: false },
      title: {
        display: true,
        text: '30 天轨迹评估趋势',
        color: 'var(--text-primary)',
        font: { size: 14, weight: 600 as const },
      },
    },
    scales: {
      y: {
        min: 0,
        max: 1,
        grid: { color: 'var(--border-subtle)' },
        ticks: { color: 'var(--text-tertiary)', font: { size: 12 } },
      },
      x: {
        grid: { color: 'var(--border-subtle)' },
        ticks: { color: 'var(--text-tertiary)', font: { size: 12 } },
      },
    },
  };

  return (
    <div className="eval-panel">
      <h3 className="eval-panel-title">轨迹评估（B7）</h3>
      {loading && <p className="eval-panel-empty">加载中…</p>}
      {error && <p className="eval-panel-empty">加载失败：{error}</p>}
      {!loading && !error && (
        <>
          {trend.length > 0 ? (
            <div className="eval-panel-chart">
              <Line data={data} options={options} />
            </div>
          ) : (
            <p className="eval-panel-empty">
              暂无评估数据。轨迹评分在会话产生 tool 调用后通过 eval_run_session 记录。
            </p>
          )}
          {runs.length > 0 && (
            <div className="eval-runs-list">
              {runs.map((r) => {
                const g = GRADE_LABEL[r.grade] ?? GRADE_LABEL.incorrect;
                return (
                  <div key={r.id} className="eval-run-row">
                    <span className={`eval-grade ${g.cls}`}>{g.text}</span>
                    <span className="eval-run-score">{r.score.toFixed(2)}</span>
                    <span className="eval-run-matcher">{r.matcher}</span>
                    <span className="eval-run-steps">{r.steps} 步</span>
                  </div>
                );
              })}
            </div>
          )}
        </>
      )}
    </div>
  );
}
