import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useNavigationStore } from '../../stores/navigationStore';
import { Button } from '../ui/Button/Button';
import type { ForgeExperienceReview, ReplayResult } from '../../types';

/**
 * 质量经验回放 (Forge experience replay) — DevWorkbench 的护城河之一。
 *
 * Forge 给低分任务创建 mandatory pending review，这些低维度教训会被沉淀进
 * 知识库（quality_failure lesson），在新任务里作为 <memory-context> 注入
 * system prompt，避免重蹈覆辙。
 *
 * 后端 `quality::experience::replay_to_knowledge` 已完整实现（dedup / 全局提升
 * promote_global / 衰减 decay / 清理 purge），且每次开 agent 时由 agents.rs 自动
 * 飞轮触发。本 section 补的是**可见性 + 手动入口**：让用户看到当前项目有哪些
 * 待回顾的低分任务，并可手动重新触发回放（强制刷新知识库）。
 *
 * - list_pending_forge_reviews(projectPath) → pending+mandatory 子集
 *   （forge 未装时后端返回 ForgeNotInstalled → 这里 catch 成友好提示）
 * - replay_forge_experience(projectPath) → {replayed, skipped, promotedGlobal}
 *   （后端容错，无 pending = no-op）
 */
function errMsg(e: unknown, fallback: string): string {
  if (typeof e === 'string') return e;
  const m = (e as { message?: string })?.message;
  return m ?? fallback;
}

export function ForgeExperienceSection() {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const [reviews, setReviews] = useState<ForgeExperienceReview[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [replaying, setReplaying] = useState(false);
  const [replayResult, setReplayResult] = useState<ReplayResult | null>(null);

  const projectPath = activeProject?.path ?? null;

  useEffect(() => {
    if (!projectPath) {
      setReviews([]);
      setError(null);
      setLoading(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setError(null);
    (async () => {
      try {
        const raw = await invoke<ForgeExperienceReview[]>('list_pending_forge_reviews', { projectPath });
        if (!cancelled) setReviews(Array.isArray(raw) ? raw : []);
      } catch (e) {
        // forge 未装时后端返回 ForgeNotInstalled；e2e mock 返回 null 时上面 coerce
        if (!cancelled) {
          setReviews([]);
          setError(errMsg(e, '读取待回顾经验失败'));
        }
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [projectPath]);

  const handleReplay = async () => {
    if (!projectPath || replaying) return;
    setReplaying(true);
    setReplayResult(null);
    setError(null);
    try {
      const result = await invoke<ReplayResult>('replay_forge_experience', { projectPath });
      setReplayResult(result);
      // 回放后刷新列表（部分 review 可能已被 forge 标记 resolved/accepted）
      const raw = await invoke<ForgeExperienceReview[]>('list_pending_forge_reviews', { projectPath }).catch(
        () => [] as ForgeExperienceReview[],
      );
      setReviews(Array.isArray(raw) ? raw : []);
    } catch (e) {
      setError(errMsg(e, '回放失败'));
    } finally {
      setReplaying(false);
    }
  };

  const lowDimTotal = reviews.reduce((n, r) => n + r.lowDimensions.length, 0);

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">质量经验回放</h3>
      <p className="settings-section-desc">
        {activeProject ? (
          <>
            项目 <code>{activeProject.name}</code> 中 Forge 标记为「待回顾」的低分任务。回放后，这些任务的低维度教训会沉淀进知识库，在新任务中自动注入，避免重蹈覆辙。
          </>
        ) : (
          '请先选择一个项目，查看该项目待回顾的质量经验。'
        )}
      </p>

      {!activeProject ? (
        <p className="muted">未选择项目</p>
      ) : (
        <>
          <div className="capability-stats">
            <div className="capability-stat">
              <span className="capability-stat-num">{reviews.length}</span>
              <span className="capability-stat-label">待回顾任务</span>
            </div>
            <div className="capability-stat">
              <span className="capability-stat-num">{lowDimTotal}</span>
              <span className="capability-stat-label">低维度教训</span>
            </div>
          </div>

          {error && (
            <p className="muted">⚠ {error}（请确认已安装 Forge CLI 并完成任务评分）</p>
          )}

          <div className="forge-action-row">
            <Button
              variant="primary"
              isLoading={replaying}
              disabled={reviews.length === 0}
              onClick={handleReplay}
            >
              {replaying ? '回放中…' : '↻ 重放到知识库'}
            </Button>
            {replayResult && (
              <span className="forge-result">
                已回放 {replayResult.replayed} 条，跳过 {replayResult.skipped} 条
                {replayResult.promotedGlobal > 0 && `，提升 ${replayResult.promotedGlobal} 条跨项目通用经验`}
              </span>
            )}
          </div>

          {loading && <div className="config-center-loading">加载中…</div>}

          <div className="capability-group" style={{ marginTop: 16 }}>
            <h4 className="capability-group-title">待回顾任务（{reviews.length}）</h4>
            {reviews.length === 0 ? (
              <p className="muted">
                {error ? '' : '暂无待回顾经验 — 当前项目没有 Forge 标记的低分任务，或回放已全部沉淀'}
              </p>
            ) : (
              <ul className="capability-list">
                {reviews.map((r) => (
                  <li key={r.taskRef} className="forge-review-item">
                    <div className="forge-review-head">
                      <code className="forge-review-task">{r.taskRef}</code>
                      <span className={`forge-grade ${r.score < 70 ? 'low' : ''}`}>
                        {r.grade} · {r.score}
                      </span>
                    </div>
                    {r.lowDimensions.length > 0 && (
                      <ul className="forge-low-dims">
                        {r.lowDimensions.map((d, i) => (
                          <li key={i}>
                            <strong>{d.dimension}</strong>（{d.score}）{d.detail ? `：${d.detail}` : ''}
                          </li>
                        ))}
                      </ul>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </div>
        </>
      )}
    </div>
  );
}
