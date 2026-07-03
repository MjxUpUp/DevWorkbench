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

/** A1 span 树的一个节点。kind="llm" 是父 LLM 调用 span（每条 trace 一个），
 *  kind="tool" 是挂在它下面的工具调用子 span。latency/status 直接取自 trace 行。 */
export interface Span {
  kind: string;
  name: string;
  latency_ms?: number;
  status?: string;
  children?: Span[];
}

/** P3 富轨迹：工具步 + 文件变更 + token 用量 + 估算成本 + span 树。
 *  preview_session_trajectory 返回。cost_cents 是 ESTIMATE_RATE_PER_M_TOKENS
 *  的粗估（token 才是真信号；真实计价在 cost 模块按 provider 算）。 */
export interface FullTrajectory {
  steps: ToolStep[];
  files_changed: string[];
  input_tokens: number;
  output_tokens: number;
  /** 粗估成本（USD¢），标注「估算」。 */
  cost_cents: number;
  span_tree: { roots: Span[] };
}

/** P6 八维 rubric 的一维。score∈[0,1]；val 是原始读数（"2 次"/"3/4"/"=0 命中"）；
 *  hard 标硬门维度（失败直接把 Q_code 归零）。 */
export interface RubricDim {
  key: string;
  label: string;
  score: number;
  val: string;
  hard?: boolean;
}

/** P6 八维可靠性判决。q_code 是加权汇总（任一硬门触发→0）；hard_gate_triggered
 *  标是否触发了 manual-intervention 硬门。 */
export interface RubricScore {
  dims: RubricDim[];
  q_code: number;
  hard_gate_triggered: boolean;
}

/** P4 平台-机制 eval 的确定性契约：期望的节点执行序 + 终态。任一字段空=不查。 */
export interface MechanismExpect {
  expect_order: string[];
  /** "done" | "failed" | "interrupted"；空=不查。 */
  expect_terminal: string;
}

/** P4 平台-机制 eval 的判决。pass=所有非空期望都命中引擎实际行为。 */
export interface MechanismVerdict {
  pass: boolean;
  actual_order: string[];
  actual_terminal: string;
  expected_order: string[];
  expected_terminal: string;
  mismatches: string[];
}

// ── P4 平台-e2e eval（数据平面：persistence → DB → 返回形状）──
/** 一个要 seed 进 eval_cases 的 case 行。 */
export interface E2ESeedCase {
  id: string;
  name: string;
  category: string;
  input_prompt: string;
  expected_steps_json?: string | null;
  negative_json?: string | null;
  /** draft=true → 未审核，应被 approved 列表排除、include_drafts 含。 */
  draft?: boolean;
}
/** 一个要 seed 进 verdicts 账本的 verdict 行。 */
export interface E2ESeedVerdict {
  gate: string;
  verdict: string;
  session_id?: string | null;
  case_id?: string | null;
}
/** seed：跑断言前灌进临时库的数据。 */
export interface E2ESeed {
  cases?: E2ESeedCase[];
  verdicts?: E2ESeedVerdict[];
}
/** 一次回放打分断言：载入 case 契约，对 stub 轨迹打分，要求该 grade。 */
export interface E2EReplayExpect {
  case_id: string;
  actual_steps: string[];
  matcher?: Matcher;
  expected_grade: 'optimal' | 'suboptimal' | 'incorrect';
}
/** 平台-e2e 的确定性契约。每个 Option=null 表示「不查」该维度。 */
export interface E2EExpect {
  approved_case_count?: number | null;
  total_case_count?: number | null;
  verdict_count_for_gate?: [string, number] | null;
  replay?: E2EReplayExpect | null;
}
/** 单条断言结果（UI 展示哪个维度过/挂）。 */
export interface E2ECheck {
  name: string;
  pass: boolean;
  detail: string;
}
/** 平台-e2e 判决。pass=所有 set 的期望都命中数据平面行为。 */
export interface E2EVerdict {
  pass: boolean;
  checks: E2ECheck[];
  mismatches: string[];
}

// ── P4·调整 平台-自审 eval（IPC 接线完整性：前端 invoke 集 vs 后端注册集）──
/** 单条自审断言（UI 展示哪个维度过/挂）。镜像 E2ECheck。 */
export interface CoverageCheck {
  name: string;
  pass: boolean;
  detail: string;
}
/** 平台-自审判决。pass=零死按钮。两集合独立 grep 客观事实，无手工契约——
 *  F（前端 invoke，构建期 manifest）vs B（后端 generate_handler! 注册）。 */
export interface CoverageVerdict {
  pass: boolean;
  frontend_count: number;
  backend_count: number;
  aligned_count: number;
  /** F\B：前端 invoke 但后端未注册 → 死按钮（FAIL）。用户点了无反应=造假。 */
  dead_buttons: string[];
  /** B\F：后端注册但前端未调用 → 死代码（WARN，不 fail）。事件/内部/未接线。 */
  dead_code: string[];
  checks: CoverageCheck[];
}

// ── P4 平台-加持 eval（开/关 DW 功能 → agent 轨迹增量）──
/** enablement 跑的 off→on 结果（镜像 L4 PairedOutcome）。 */
export type EnablementOutcome = 'improvement' | 'regression' | 'no_change';
/** enablement 切换的 DW 功能。当前仅 skills。 */
export type EnablementFeature = 'skills';
/**
 * 开/关一个 DW 功能是否真改善了 agent。attribution: CLEAR=可归因增益
 * （ON 闭合了到 expected 的缺口）/ BRAKE=无因果链的增益 / null=无增益可归因。
 * 需 live provider key（两次真 agent 跑）。
 */
export interface EnablementVerdict {
  feature: EnablementFeature;
  outcome: EnablementOutcome;
  attribution: Attribution | null;
  off_score: number;
  on_score: number;
  reason: string;
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
   *  需 live provider key（agent 真跑）。working_dir 是被测工作区（只读沙箱）。
   *  enableSkills: P4 功能开关矩阵的 skills 维（None=默认 true）。 */
  runReplay: (
    caseId: string,
    workingDir: string,
    matcher: Matcher,
    model?: string,
    enableSkills?: boolean,
  ) =>
    invoke<ReplayVerdict>('run_eval_replay', {
      caseId,
      workingDir,
      matcher,
      model,
      enableSkills,
    }),

  // ----- P3 会话→Case 预览 -----
  /** 预览一个会话重建的富轨迹（不落库）：步 + 文件 + token + 成本 + span 树。
   *  P3 向导展示，让用户在存草稿前 curate；A1 也用它渲染 span 树。 */
  previewTrajectory: (sessionId: string) =>
    invoke<FullTrajectory>('preview_session_trajectory', { sessionId }),

  // ----- P6 八维 rubric -----
  /** 对一个 session×case 算 8 维 AgentX 可靠性 rubric（纯函数，无 LLM 自评）。
   *  装配 RubricInput 全部来自已记录事实（trace 步/失败、activity 文件、case 契约）。 */
  scoreRubric: (sessionId: string, caseId: string, matcher: Matcher = 'exact_match') =>
    invoke<RubricScore>('score_eval_rubric', { sessionId, caseId, matcher }),

  // ----- P4 平台-机制 eval -----
  /** 跑一个平台-机制 case：编译 YAML 工作流，stub executor 驱动，对比节点序+终态。
   *  无 LLM——判决是引擎 GraphEvent 序的客观事实（反刷分 #1）。 */
  runPlatformMechanism: (
    graphYaml: string,
    inputJson: unknown,
    expect: MechanismExpect,
  ) =>
    invoke<MechanismVerdict>('eval_platform_mechanism', { graphYaml, inputJson, expect }),

  // ----- P4 平台-e2e eval（数据平面）-----
  /** 跑一个平台-e2e case：临时内存库（真 schema）→ seed → 对真持久化/逻辑函数
   *  断言（draft 过滤/回放打分/gate 计数）。无 LLM/无浏览器，判决=数据契约的
   *  客观事实；浏览器渲染层由 playwright eval.spec.ts 守护。 */
  runPlatformE2e: (seed: E2ESeed, expect: E2EExpect) =>
    invoke<E2EVerdict>('eval_platform_e2e', { seed, expect }),

  // ----- P4 平台-加持 eval -----
  /** 跑一个平台-加持 case：skills 关→开两次真 agent 回放，compare_paired diff。
   *  增益须闭合到 expected 缺口才算 CLEAR，否则 BRAKE。需 live provider key。 */
  runEnablement: (caseId: string, workingDir: string, matcher: Matcher, model?: string) =>
    invoke<EnablementVerdict>('run_eval_enablement', { caseId, workingDir, matcher, model }),

  // ----- P4·调整 平台-自审 eval（IPC 接线）-----
  /** 自审 dev workbench 自身：前端 invoke 集合（构建期 manifest 传入）vs 后端
   *  generate_handler! 注册集合（后端 include_str! lib.rs 解析）。dead_buttons
   *  （前端调了后端没注册）= FAIL；dead_code（后端注册前端没调）= WARN。反刷分
   *  自审——评测系统用它审计 dev workbench 自身不造假。 */
  runPlatformCoverage: (frontendInvokes: string[]) =>
    invoke<CoverageVerdict>('eval_platform_coverage', { frontendInvokes }),
};
