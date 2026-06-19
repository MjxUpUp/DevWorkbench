import { useState } from 'react';
import {
  missionApi,
  type MissionLoadResult,
  type MissionStatusView,
  type MissionState,
} from '../../utils/missionApi';

/**
 * D4 Mission / Plan-Apply 二阶段 — settings section that makes the four
 * `mission_*` backend commands reachable (they had zero frontend callers). The
 * lifecycle: Phase 1 the agent writes `prd.json` in plan mode → here the user
 * `load`s + validates it → `apply` flips to Phase 2 (executing) → `status`
 * polls the story pass count. `missionId` is a free-form id the user enters
 * (e.g. `mission-2026-06-20`); a richer "start mission from a goal" entry that
 * generates the id + spawns the plan-mode session is deferred — this is the
 * control/debug surface that closes the zero-caller gap.
 */
export function MissionSection() {
  const [missionId, setMissionId] = useState('');
  const [state, setState] = useState<MissionState | null>(null);
  const [loadResult, setLoadResult] = useState<MissionLoadResult | null>(null);
  const [status, setStatus] = useState<MissionStatusView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run<T>(fn: () => Promise<T>, onOk: (r: T) => void) {
    if (!missionId.trim()) return;
    setBusy(true);
    setError(null);
    try {
      onOk(await fn());
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const onInit = () => run(() => missionApi.init(missionId.trim()), setState);
  const onLoad = () => run(() => missionApi.loadPrd(missionId.trim()), setLoadResult);
  const onApply = () =>
    run(() => missionApi.apply(missionId.trim()), (s) => {
      setState(s);
      // apply flips to executing — refresh the live status right away.
      void run(() => missionApi.status(missionId.trim()), setStatus);
    });
  const onStatus = () => run(() => missionApi.status(missionId.trim()), setStatus);

  const disabled = busy || !missionId.trim();
  const phaseLabel: Record<MissionState['currentPhase'], string> = {
    plan: 'Phase 1 · 编写 PRD',
    executing: 'Phase 2 · 执行验收',
    completed: '已完成',
    max_iterations_reached: '达到迭代上限',
  };

  return (
    <div className="mission-section">
      <h2 className="section-title">任务编排（Mission · Plan-Apply 二阶段）</h2>
      <p className="section-desc">
        Phase 1：智能体在 plan 模式编写 <code>prd.json</code>；Phase 2：apply 后 controller-only
        执行，逐 story 验收。对齐 QwenPaw Mission Mode + Forge 三门禁。
      </p>

      <div className="mission-input-row">
        <input
          value={missionId}
          onChange={(e) => setMissionId(e.target.value)}
          placeholder="mission id（如 mission-2026-06-20）"
          className="mission-id-input"
        />
        <button type="button" onClick={onInit} disabled={disabled}>
          init
        </button>
        <button type="button" onClick={onLoad} disabled={disabled}>
          load PRD
        </button>
        <button
          type="button"
          onClick={onApply}
          disabled={disabled || !loadResult?.valid}
          title={loadResult?.valid ? 'flip to Phase 2' : '需先 load 且 PRD 校验通过'}
        >
          apply
        </button>
        <button type="button" onClick={onStatus} disabled={disabled}>
          status
        </button>
      </div>

      {error && <p className="mission-error">{error}</p>}

      {state && (
        <div className="mission-state">
          阶段：<strong>{phaseLabel[state.currentPhase]}</strong> · 迭代 {state.iteration}/
          {state.maxIterations}
        </div>
      )}

      {loadResult && (
        <div className="mission-prd">
          PRD 校验：
          {loadResult.valid ? (
            '✅ 通过，可 apply'
          ) : (
            <span>❌ {loadResult.problems.join('；') || '校验失败'}</span>
          )}
          {loadResult.corrupted && <span className="mission-warn"> · ⚠ PRD 文件损坏</span>}
        </div>
      )}

      {status && (
        <div className="mission-status">
          验收：{status.passed}/{status.total} stories
          {status.corrupted && <span className="mission-warn"> · ⚠ PRD 损坏</span>}
        </div>
      )}
    </div>
  );
}
