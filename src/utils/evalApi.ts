import { invoke } from '@tauri-apps/api/core';

/**
 * B7 trajectory-eval frontend API — typed wrappers over the `eval_*` Tauri
 * commands. The flow: reconstruct a session's tool-call trajectory from its
 * LLM traces, score it against an optional reference (golden path) under one
 * of three matchers, persist the run, and read back the daily regression
 * curve. Mirrors the OpenAI Agents SDK `trajectory-evaluation` rubric
 * (optimal / suboptimal / incorrect).
 *
 * Rust command params are snake_case (`session_id`); Tauri converts to the
 * camelCase keys passed here (`sessionId`). Wire rows are snake_case to match
 * the backend `EvalRunRow` serialization.
 */
export type Matcher = 'exact_match' | 'in_order' | 'any_order';
export type Grade = 'optimal' | 'suboptimal' | 'incorrect';

export interface EvalRunRow {
  id: string;
  session_id: string | null;
  conversation_id: string | null;
  matcher: string;
  score: number;
  grade: Grade;
  steps: number;
  created_at: string;
}

export interface TrendPoint {
  /** UTC day, `YYYY-MM-DD`. */
  date: string;
  /** Mean score across runs in this bucket, [0, 1]. */
  avg_score: number;
  /** How many runs landed in this bucket. */
  count: number;
}

export const evalApi = {
  /**
   * Score a session's trajectory and persist the run. `matcher` controls how
   * strictly `reference` (the expected tool-name sequence) must be followed;
   * omit `reference` for a reference-free heuristic (redundancy-based). Returns
   * the stored row.
   */
  runSession: (
    sessionId: string,
    matcher: Matcher = 'exact_match',
    reference?: string[],
  ) => invoke<EvalRunRow>('eval_run_session', { sessionId, matcher, reference }),

  /** List eval runs, newest-first. Scope to a session when `sessionId` is set. */
  listRuns: (sessionId?: string, limit?: number) =>
    invoke<EvalRunRow[]>('list_eval_runs', { sessionId, limit }),

  /** Daily regression curve over the last `days` days (default 30), ASC by date. */
  trend: (days?: number) => invoke<TrendPoint[]>('eval_trend', { days }),
};
