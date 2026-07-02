import { invoke } from '@tauri-apps/api/core';

/**
 * 评测/回放面板前端 API — 反刷分三原则的数据平面。
 *
 * 三原则（核心立场，源 AgentX 论文 SGPO 配对回放）：
 * - **客观事实代码判** — 预期步骤/observables 是确定性提取的客观轨迹
 *   （extract_trajectory），只有 expected_output 才走 LLM 判。回放分数是
 *   纯序列匹配，LLM 不给自己的产出打分。
 * - **因果归因** — attribution: CLEAR（可验证因果链）/ UNCLEAR / BRAKE。
 *   未归因的增益 = BRAKE，不记录为赢。
 * - **配对回放** — 策略/prompt/平台更新须过新旧 trace 配对对比（L4）。
 *
 * 后端命令是 snake_case（`session_id`）；Tauri 把这里的 camelCase 键
 * （`sessionId`）转成 snake_case。Wire 行字段是 snake_case，对齐后端
 * `VerdictRow` / `EvalCaseRow` / `EvalRunRow` 的 serde 默认序列化。
 */
export type Matcher = 'exact_match' | 'in_order' | 'any_order';
export type Grade = 'optimal' | 'suboptimal' | 'incorrect';
/** 反刷分归因：CLEAR=可验证因果链 / UNCLEAR=有增益但归因不全 / BRAKE=未归因增益，刹车 */
export type Attribution = 'CLEAR' | 'UNCLEAR' | 'BRAKE';

/** L1 verdict ledger 一行（verify/honesty/forge/circuit-breaker/eval 通用）。 */
export interface VerdictRow {
  id: string;
  session_id: string | null;
  case_id: string | null;
  gate: string;
  verdict: string;
  attribution: string | null;
  report: string | null;
  commit_sha: string | null;
  created_at: string;
}

/** 一条重建轨迹里的工具调用步（preview_session_trajectory 返回）。 */
export interface ToolStep {
  name: string;
  /** "error" 若承载该调用的 trace 非 2xx 或有 error_kind；否则 null（成功）。 */
  status: string | null;
}

/** L2 eval case —— 回放/配对跑的确定性契约。 */
export interface EvalCaseRow {
  id: string;
  name: string;
  category: string;
  input_prompt: string;
  expected_steps_json: string | null;
  expected_output: string | null;
  expected_observables_json: string | null;
  negative_json: string | null;
  source_session_id: string | null;
  commit_sha: string | null;
  /** draft=1 → 轨迹冻结但未审核，不能 anchor 配对（防 agent 自我背书）。 */
  draft: number;
  created_at: string;
}

/** L3 单次回放的判决（run_eval_replay 返回；同时落一条 gate="eval" verdict）。 */
export interface ReplayVerdict {
  score: number;
  grade: Grade;
  verdict: string;
  attribution: string | null;
  negative_violated: boolean;
  reason: string;
}

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

/** create_eval_case 的入参（id/created_at 服务端生成，draft 默认 false）。 */
export interface CreateCaseInput {
  name: string;
  category: string;
  inputPrompt: string;
  expectedStepsJson?: string;
  expectedOutput?: string;
  expectedObservablesJson?: string;
  negativeJson?: string;
  sourceSessionId?: string;
  commitSha?: string;
  /** true = 存为未审核草稿（从轨迹转出来时）。默认 false。 */
  draft?: boolean;
}

/** update_eval_case 的入参（input_prompt 锁定，不在此）。 */
export interface UpdateCaseInput {
  name: string;
  category: string;
  expectedStepsJson?: string;
  expectedOutput?: string;
  expectedObservablesJson?: string;
  negativeJson?: string;
}

export const evalApi = {
  // ----- 旧 B7 trajectory-eval（保留：单会话打分 + 趋势）-----
  /** 对一个会话的轨迹打分并落库。留空 reference 走无参考冗余启发式。 */
  runSession: (
    sessionId: string,
    matcher: Matcher = 'exact_match',
    reference?: string[],
  ) => invoke<EvalRunRow>('eval_run_session', { sessionId, matcher, reference }),

  /** 列 eval runs，新优先；sessionId 可选范围。 */
  listRuns: (sessionId?: string, limit?: number) =>
    invoke<EvalRunRow[]>('list_eval_runs', { sessionId, limit }),

  /** 最近 days 天（默认 30）的日级回归曲线，按日期升序。 */
  trend: (days?: number) => invoke<TrendPoint[]>('eval_trend', { days }),

  // ----- L1 verdict ledger（V1 视图）-----
  /** 列判决（L1 账本），新优先。sessionId/gate/caseId 任选 AND 组合过滤。 */
  listVerdicts: (params: {
    sessionId?: string;
    gate?: string;
    caseId?: string;
    limit?: number;
  } = {}) =>
    invoke<VerdictRow[]>('list_verdicts', {
      sessionId: params.sessionId,
      gate: params.gate,
      caseId: params.caseId,
      limit: params.limit,
    }),

  // ----- L2 eval cases（P1/P2/P3）-----
  /** 列 case。默认只列已审核（排除 draft）；includeDrafts=true 看草稿。 */
  listCases: (params: { category?: string; includeDrafts?: boolean; limit?: number } = {}) =>
    invoke<EvalCaseRow[]>('list_eval_cases', {
      category: params.category,
      includeDrafts: params.includeDrafts,
      limit: params.limit,
    }),

  /** 按 id 取一条 case（P2 详情）。 */
  getCase: (id: string) => invoke<EvalCaseRow | null>('get_eval_case', { id }),

  /** 审核一条草稿 case（draft→0，才能 anchor 配对）。返回受影响行数。 */
  approveCase: (id: string) => invoke<number>('approve_eval_case', { id }),

  /** 新建 case。返回新 id。 */
  createCase: (input: CreateCaseInput) => invoke<string>('create_eval_case', { input }),

  /** 更新 case 的可编辑契约字段（input_prompt 锁定不传，C1 只读）。返回受影响行数。 */
  updateCase: (id: string, input: UpdateCaseInput) =>
    invoke<number>('update_eval_case', { id, input }),

  // ----- L3 replay（P4 触发 / P5 单次视图）-----
  /** 跑一次回放：加载 case → Plan 沙箱 agent 跑 input_prompt → 轨迹打分 → 落 verdict。
   *  需 live provider key（agent 真跑）。working_dir 是被测工作区（只读沙箱）。 */
  runReplay: (caseId: string, workingDir: string, matcher: Matcher, model?: string) =>
    invoke<ReplayVerdict>('run_eval_replay', { caseId, workingDir, matcher, model }),

  // ----- P3 会话→Case 预览 -----
  /** 预览一个会话重建的工具轨迹（不落库）。P3 向导展示，让用户在存草稿前 curate。 */
  previewTrajectory: (sessionId: string) =>
    invoke<ToolStep[]>('preview_session_trajectory', { sessionId }),
};
