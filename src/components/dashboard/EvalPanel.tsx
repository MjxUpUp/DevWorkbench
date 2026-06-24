import { useCallback, useEffect, useState } from 'react';
import { Line } from 'react-chartjs-2';
import { evalApi, type TrendPoint, type EvalRunRow, type Grade, type Matcher } from '../../utils/evalApi';
import { useAgentStore } from '../../stores/agentStore';
import { Button } from '../ui/Button/Button';

/**
 * B7 trajectory-eval panel — the user-visible half of the eval pipeline. Shows
 * the daily avg-score regression curve (`eval_trend`) and the most recent
 * scored runs (`list_runs`), AND the `run_session` trigger that actually scores
 * a session: pick a finished session, choose a matcher, optionally annotate a
 * golden reference (expected tool-name sequence). Mirrors the OpenAI Agents SDK
 * `trajectory-evaluation` rubric: optimal / suboptimal / incorrect. Sits next
 * to `QualityHistory` in the Usage Stats section.
 */
const GRADE_LABEL: Record<Grade, { text: string; cls: string }> = {
  optimal: { text: '最优', cls: 'eval-grade-optimal' },
  suboptimal: { text: '次优', cls: 'eval-grade-suboptimal' },
  incorrect: { text: '错误', cls: 'eval-grade-incorrect' },
};

const MATCHER_LABEL: Record<Matcher, string> = {
  exact_match: '精确匹配（顺序+无多余）',
  in_order: '子序列（顺序对即可）',
  any_order: '任意顺序（集合相等）',
};

const MATCHERS: Matcher[] = ['exact_match', 'in_order', 'any_order'];

function parseReference(text: string): string[] | undefined {
  const parts = text
    .split(/[\n,]/)
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
  return parts.length > 0 ? parts : undefined;
}

export function EvalPanel() {
  const sessions = useAgentStore((s) => s.sessions);
  // Only finished sessions have finalized traces worth scoring; a `running`
  // session's trajectory is still mid-stream.
  const finishedSessions = sessions.filter(
    (s) => s.status === 'completed' || s.status === 'failed',
  );

  const [trend, setTrend] = useState<TrendPoint[]>([]);
  const [runs, setRuns] = useState<EvalRunRow[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const [selectedSessionId, setSelectedSessionId] = useState('');
  const [matcher, setMatcher] = useState<Matcher>('exact_match');
  const [referenceText, setReferenceText] = useState('');
  const [running, setRunning] = useState(false);
  const [runError, setRunError] = useState<string | null>(null);
  const [lastRun, setLastRun] = useState<EvalRunRow | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const [t, r] = await Promise.all([evalApi.trend(30), evalApi.listRuns(undefined, 20)]);
      setTrend(t);
      setRuns(r);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        await refresh();
      } finally {
        if (cancelled) return;
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [refresh]);

  // Default-select the most recent finished session once the list lands.
  useEffect(() => {
    if (!selectedSessionId && finishedSessions.length > 0) {
      setSelectedSessionId(finishedSessions[0].id);
    }
  }, [finishedSessions, selectedSessionId]);

  async function onRun() {
    if (!selectedSessionId) return;
    setRunning(true);
    setRunError(null);
    try {
      const row = await evalApi.runSession(
        selectedSessionId,
        matcher,
        parseReference(referenceText),
      );
      setLastRun(row);
      await refresh();
    } catch (e) {
      setRunError(String(e));
    } finally {
      setRunning(false);
    }
  }

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

      <div className="eval-trigger">
        <div className="eval-trigger-row">
          <label className="eval-trigger-label">
            会话
            <select
              value={selectedSessionId}
              onChange={(e) => setSelectedSessionId(e.target.value)}
              disabled={running || finishedSessions.length === 0}
              className="eval-trigger-select"
            >
              {finishedSessions.length === 0 && <option value="">暂无已完成的会话</option>}
              {finishedSessions.map((s) => {
                const snippet = s.prompt.slice(0, 40) || '(无提示词)';
                return (
                  <option key={s.id} value={s.id}>
                    {s.agentType} · {snippet} {s.status === 'failed' ? '· 失败' : ''}
                  </option>
                );
              })}
            </select>
          </label>
          <label className="eval-trigger-label">
            匹配器
            <select
              value={matcher}
              onChange={(e) => setMatcher(e.target.value as Matcher)}
              disabled={running}
              className="eval-trigger-select"
            >
              {MATCHERS.map((m) => (
                <option key={m} value={m}>
                  {MATCHER_LABEL[m]}
                </option>
              ))}
            </select>
          </label>
        </div>
        <label className="eval-trigger-label eval-trigger-reference">
          金标准轨迹（可选，每行一个工具名；留空走无参考冗余启发式）
          <textarea
            value={referenceText}
            onChange={(e) => setReferenceText(e.target.value)}
            disabled={running}
            placeholder={'Read\nGrep\nBash\n...'}
            rows={3}
            className="eval-trigger-textarea"
          />
        </label>
        <Button
          variant="primary"
          onClick={onRun}
          disabled={running || !selectedSessionId}
        >
          {running ? '评估中…' : '运行评估'}
        </Button>
        {runError && <p className="eval-panel-empty">评估失败：{runError}</p>}
        {lastRun && !runError && (
          <p className="eval-run-last">
            本次：{GRADE_LABEL[lastRun.grade].text} · 得分 {lastRun.score.toFixed(2)} ·{' '}
            {lastRun.steps} 步
          </p>
        )}
      </div>

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
              暂无评估数据。选择一个已完成的会话并「运行评估」即可生成首条轨迹评分。
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
