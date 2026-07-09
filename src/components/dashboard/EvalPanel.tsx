import { useCallback, useEffect, useMemo, useState } from 'react';
import { Line } from 'react-chartjs-2';
import {
  evalApi,
  type TrendPoint,
  type VerdictRow,
  type EvalCaseRow,
  type ReplayVerdict,
  type ToolStep,
  type FullTrajectory,
  type Span,
  type RubricScore,
  type E2ESeed,
  type E2EExpect,
  type E2EVerdict,
  type EnablementVerdict,
  type CoverageVerdict,
  type Matcher,
  type CreateCaseInput,
} from '../../utils/evalApi';
import { useAgentStore } from '../../stores/agentStore';
import { useNavigationStore } from '../../stores/navigationStore';
import { INVOKED_COMMANDS } from '../../generated/invoked-commands';
import { Button } from '../ui/Button/Button';

/**
 * 评测/回放面板 —— 反刷分三原则的可见窗口。原型 prototype/eval-panel.html
 * 12/12 确认。左侧 P/V/F/A 四组导航，右侧单功能视图，共享同一份
 * cases / verdicts / trend 数据（一次加载，写操作后 refresh）。
 *
 * 真实数据接线（后端已落地）：P1/P2/P3（L2 cases）、V1（L1 verdicts）、
 * F1（eval_trend）、P4 agent 类（L3 run_eval_replay）、P5（case 的 eval
 * verdicts 派生）、V2（FAIL 判决派生失败资产）、V3（report 派生归因链）、
 * F2（counts 派生）。
 * 诚实缺口：P6 八维 rubric（仅 3 态 grade，8 维待 scoring.rs 扩展）、
 * A1 OTel（零起步）—— 明确标注「未接入」，不造假数据。
 */
type FeatureId = 'P1' | 'P2' | 'P3' | 'P4' | 'P5' | 'P6' | 'V1' | 'V2' | 'V3' | 'F1' | 'F2' | 'A1' | 'SA';

const GROUPS: { title: string; items: FeatureId[] }[] = [
  { title: 'P · Case 池 & 回放', items: ['P1', 'P2', 'P3', 'P4', 'P5', 'P6'] },
  { title: 'V · 判决 & 失败资产', items: ['V1', 'V2', 'V3'] },
  { title: 'F · 飞轮', items: ['F1', 'F2'] },
  { title: 'A · OTel', items: ['A1'] },
  { title: 'SA · 平台自审', items: ['SA'] },
];

const FEATURE_TITLE: Record<FeatureId, string> = {
  P1: 'Case 池列表',
  P2: 'Case 详情 / 编辑',
  P3: '历史会话 → Case',
  P4: '评测任务配置',
  P5: 'Paired 对比视图',
  P6: '每步 rubric 评分卡',
  V1: 'Verdicts 查询',
  V2: '失败资产',
  V3: '模糊 FAIL → UI 提示',
  F1: '回归曲线',
  F2: '飞轮闭环可视化',
  A1: '配对 Trace 树（OTel）',
  SA: 'IPC 接线自审（评测 dev workbench 本身）',
};

const MATCHERS: Matcher[] = ['exact_match', 'in_order', 'any_order'];
const MATCHER_LABEL: Record<Matcher, string> = {
  exact_match: '精确匹配（顺序+无多余）',
  in_order: '子序列（顺序对即可）',
  any_order: '任意顺序（集合相等）',
};

// ── 共享数据 ──
interface EvalData {
  cases: EvalCaseRow[];
  verdicts: VerdictRow[];
  trend: TrendPoint[];
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
}

function useEvalData(): EvalData {
  const [cases, setCases] = useState<EvalCaseRow[]>([]);
  const [verdicts, setVerdicts] = useState<VerdictRow[]>([]);
  const [trend, setTrend] = useState<TrendPoint[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      // includeDrafts: P1 编辑视图需要看到草稿（probe）；replay 路径自己在
      // 前端按 draft 过滤，不全靠后端默认排除。
      const [c, v, t] = await Promise.all([
        evalApi.listCases({ includeDrafts: true, limit: 200 }),
        evalApi.listVerdicts({ limit: 300 }),
        evalApi.trend(30),
      ]);
      setCases(c);
      setVerdicts(v);
      setTrend(t);
      setError(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  return { cases, verdicts, trend, loading, error, refresh };
}

// ── 小工具 ──
function verdictTarget(v: VerdictRow, caseName: (id: string | null) => string): string {
  if (v.case_id) return `Case · ${caseName(v.case_id)}`;
  if (v.session_id) return `session #${v.session_id.slice(0, 8)}`;
  if (v.commit_sha) return `commit ${v.commit_sha.slice(0, 7)}`;
  return v.gate === 'circuit-breaker' ? '主机熔断' : '—';
}

function parseSteps(json: string | null): ToolStep[] {
  if (!json) return [];
  try {
    const parsed = JSON.parse(json) as ToolStep[];
    return Array.isArray(parsed) ? parsed : [];
  } catch {
    return [];
  }
}

/// Strict JSON-array check for P2 save validation. expected_steps_json +
/// negative_json are machine-consumed by score_eval_rubric's parse_names (which
/// expects a JSON array); a non-array value there would silently score wrong.
/// null/empty = "no constraint" (valid). Block the save with a real error
/// instead of letting a malformed contract through (反刷分: the contract must
/// be well-formed or it can't anchor a replay).
function isArrayJson(s: string | null): boolean {
  if (!s) return true;
  try {
    return Array.isArray(JSON.parse(s));
  } catch {
    return false;
  }
}

function fmtTime(iso: string): string {
  try {
    return new Date(iso).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' });
  } catch {
    return iso.slice(11, 16);
  }
}

// ── 归因/状态徽章 ──
function AttributionBadge({ value }: { value: string | null }) {
  if (!value) return <span className="eval-badge eval-badge-muted">—</span>;
  const cls =
    value === 'CLEAR' ? 'eval-badge-clear' : value === 'BRAKE' ? 'eval-badge-brake' : 'eval-badge-unclear';
  return <span className={`eval-badge ${cls}`}>{value}</span>;
}

function GateBadge({ gate }: { gate: string }) {
  const cls =
    gate === 'circuit-breaker'
      ? 'eval-gate-warn'
      : ['verify', 'forge'].includes(gate)
        ? 'eval-gate-pass'
        : gate === 'honesty'
          ? 'eval-gate-fail'
          : 'eval-gate-info';
  return <span className={`eval-gate ${cls}`}>{gate}</span>;
}

function VerdictBadge({ verdict }: { verdict: string }) {
  const isPass = verdict === 'PASS' || verdict === 'RESET' || /^\d/.test(verdict);
  const cls = isPass ? 'eval-badge-clear' : verdict === 'FAIL' ? 'eval-badge-brake' : 'eval-badge-unclear';
  return <span className={`eval-badge ${cls}`}>{verdict}</span>;
}

// ── 主组件 ──
export function EvalPanel() {
  const data = useEvalData();
  // P1 引用失效检测：case 的 source_session_id 是否仍在运行记录里。
  const sessions = useAgentStore((s) => s.sessions);
  const [selected, setSelected] = useState<FeatureId>('P1');
  const [selectedCaseId, setSelectedCaseId] = useState<string | null>(null);

  const caseName = useCallback(
    (id: string | null) => {
      if (!id) return '?';
      return data.cases.find((c) => c.id === id)?.name ?? id.slice(0, 8);
    },
    [data.cases],
  );

  const failCount = useMemo(
    () =>
      data.verdicts.filter(
        (v) => v.verdict === 'FAIL' || v.verdict === 'TRIPPED' || v.attribution === 'BRAKE',
      ).length,
    [data.verdicts],
  );

  const counts: Partial<Record<FeatureId, number>> = {
    P1: data.cases.length,
    V1: data.verdicts.length,
    V2: failCount,
  };

  function pickCase(id: string) {
    setSelectedCaseId(id);
    setSelected('P2');
  }

  return (
    <div className="eval-shell">
      <aside className="eval-sidebar">
        <div className="eval-shell-title">评测 / 回放</div>
        {GROUPS.map((g) => (
          <div key={g.title} className="eval-nav-group">
            <div className="eval-nav-label">{g.title}</div>
            {g.items.map((id) => (
              <button
                key={id}
                className={`eval-nav-item ${selected === id ? 'active' : ''}`}
                onClick={() => setSelected(id)}
                data-testid={`eval-nav-${id}`}
              >
                <span className="eval-nav-id">{id}</span>
                <span className="eval-nav-text">{FEATURE_TITLE[id]}</span>
                {counts[id] != null && <span className="eval-nav-count">{counts[id]}</span>}
              </button>
            ))}
          </div>
        ))}
      </aside>

      <section className="eval-main">
        <header className="eval-feature-head">
          <span className="eval-feature-num">{selected}</span>
          <h3 className="eval-feature-title" data-testid="eval-feature-title">
            {FEATURE_TITLE[selected]}
          </h3>
          <Button variant="ghost" onClick={() => void data.refresh()} disabled={data.loading}>
            {data.loading ? '刷新中…' : '刷新'}
          </Button>
        </header>

        {data.error && <p className="eval-empty">加载失败：{data.error}</p>}

        {selected === 'P1' && (
          <CasePool cases={data.cases} sessions={sessions} onSelect={pickCase} onNewCase={() => setSelected('P3')} />
        )}
        {selected === 'P2' && (
          <CaseDetail caseId={selectedCaseId} cases={data.cases} onChanged={data.refresh} />
        )}
        {selected === 'P3' && <SessionToCase onChanged={data.refresh} onDone={() => setSelected('P1')} />}
        {selected === 'P4' && (
          <ReplayLaunch cases={data.cases} onReplayed={data.refresh} pickCase={pickCase} />
        )}
        {selected === 'P5' && <PairedCompare cases={data.cases} verdicts={data.verdicts} />}
        {selected === 'P6' && <RubricCard verdicts={data.verdicts} caseName={caseName} />}
        {selected === 'V1' && <VerdictsLedger verdicts={data.verdicts} caseName={caseName} />}
        {selected === 'V2' && <FailureAssets verdicts={data.verdicts} caseName={caseName} />}
        {selected === 'V3' && <AmbiguousFail verdicts={data.verdicts} caseName={caseName} />}
        {selected === 'F1' && <RegressionCurve trend={data.trend} verdicts={data.verdicts} />}
        {selected === 'F2' && (
          <Flywheel caseCount={data.cases.length} verdictCount={data.verdicts.length} failCount={failCount} />
        )}
        {selected === 'A1' && <OtelTraces cases={data.cases} verdicts={data.verdicts} />}
        {selected === 'SA' && <CoverageSelfAudit />}
      </section>
    </div>
  );
}

// ── SA 平台自审（IPC 接线完整性：dev workbench 自评测）──
// 反刷分核心立场的自我应用：评测系统用它审计 dev workbench 自身——前端 invoke
// 集合（构建期 grep src/ → INVOKED_COMMANDS）vs 后端 generate_handler! 注册集合
// （include_str! lib.rs）。F\B=死按钮 FAIL，B\F=死代码 WARN。两集合独立 grep，
// 零手工契约，不可自我标榜。
function CoverageSelfAudit() {
  const [verdict, setVerdict] = useState<CoverageVerdict | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      // INVOKED_COMMANDS 是构建期生成的 F 集合真相源（readonly），展开传入后端比对。
      const v = await evalApi.runPlatformCoverage([...INVOKED_COMMANDS]);
      setVerdict(v);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void run();
  }, [run]);

  return (
    <div className="eval-card">
      <div className="eval-toolbar">
        <span className="eval-meta mono">
          前端 invoke {verdict?.frontend_count ?? '…'} · 后端注册 {verdict?.backend_count ?? '…'} · 对齐{' '}
          {verdict?.aligned_count ?? '…'}
        </span>
        <Button variant="primary" onClick={() => void run()} disabled={loading}>
          {loading ? '审计中…' : '重新审计'}
        </Button>
      </div>
      <p className="eval-help">
        反刷分自审：F（前端 invoke，构建期 grep src/）vs B（后端 generate_handler! 注册，include_str! lib.rs）。
        死按钮（前端调了后端没注册）= <strong>FAIL</strong>；死代码（后端注册前端没调）= WARN。
        两集合独立 grep 客观事实，零手工契约。
      </p>
      {error && <p className="eval-empty">审计失败：{error}</p>}
      {verdict && (
        <>
          <div
            className={`eval-coverage-verdict ${verdict.pass ? 'pass' : 'fail'}`}
            data-testid="coverage-verdict"
          >
            {verdict.pass
              ? `✓ PASS · 零死按钮（${verdict.aligned_count} 命令对齐）`
              : `✗ FAIL · ${verdict.dead_buttons.length} 个死按钮`}
          </div>
          {verdict.dead_buttons.length > 0 && (
            <div className="eval-coverage-section" data-testid="coverage-dead-buttons">
              <h4 className="eval-coverage-h fail">死按钮（前端 invoke 但后端未注册 → FAIL）</h4>
              <ul className="eval-coverage-list">
                {verdict.dead_buttons.map((c) => (
                  <li key={c} className="mono">
                    ✗ {c}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {verdict.dead_code.length > 0 && (
            <div className="eval-coverage-section" data-testid="coverage-dead-code">
              <h4 className="eval-coverage-h warn">
                死代码（后端注册但前端未调用 · {verdict.dead_code.length} · WARN）
              </h4>
              <ul className="eval-coverage-list">
                {verdict.dead_code.map((c) => (
                  <li key={c} className="mono">
                    · {c}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </>
      )}
    </div>
  );
}

// ── P1 Case 池 ──
function CasePool({
  cases,
  sessions,
  onSelect,
  onNewCase,
}: {
  cases: EvalCaseRow[];
  sessions: { id: string }[];
  onSelect: (id: string) => void;
  onNewCase: () => void;
}) {
  const [q, setQ] = useState('');
  const [filter, setFilter] = useState<'all' | 'ready' | 'probe' | 'backlog'>('all');

  const status = (c: EvalCaseRow): 'ready' | 'probe' | 'backlog' =>
    c.category === 'backlog' ? 'backlog' : c.draft ? 'probe' : 'ready';

  // P1 边缘态：case 引用的 source session 已不在运行记录里 → 来源失效（需重绑/归档）。
  const liveSessionIds = useMemo(() => new Set(sessions.map((s) => s.id)), [sessions]);

  const rows = cases.filter((c) => {
    const s = status(c);
    if (filter !== 'all' && s !== filter) return false;
    if (!q) return true;
    const hay = `${c.name} ${c.source_session_id ?? ''} ${c.commit_sha ?? ''} ${c.category}`.toLowerCase();
    return hay.includes(q.toLowerCase());
  });

  return (
    <div className="eval-card">
      <div className="eval-toolbar">
        <input
          className="eval-input"
          placeholder="搜索 case / session_id / commit"
          value={q}
          onChange={(e) => setQ(e.target.value)}
        />
        <select className="eval-input" value={filter} onChange={(e) => setFilter(e.target.value as typeof filter)}>
          <option value="all">全部状态</option>
          <option value="ready">ready（已审核）</option>
          <option value="probe">probe（草稿待审）</option>
          <option value="backlog">backlog（远期）</option>
        </select>
        <Button variant="primary" onClick={onNewCase}>
          + 从会话转 Case
        </Button>
      </div>

      {rows.length === 0 ? (
        <p className="eval-empty">评测池为空。从历史会话转第一个 Case →</p>
      ) : (
        <div className="eval-case-list">
          {rows.map((c) => {
            const s = status(c);
            const stale = c.source_session_id != null && !liveSessionIds.has(c.source_session_id);
            return (
              <button key={c.id} className={`eval-case-row ${stale ? 'stale' : ''}`} onClick={() => onSelect(c.id)}>
                <span className={`eval-badge eval-badge-${s === 'ready' ? 'clear' : s === 'probe' ? 'unclear' : 'muted'}`}>
                  {s}
                </span>
                <span className="eval-case-title">{c.name}</span>
                {stale ? (
                  <span
                    className="eval-badge eval-badge-unclear"
                    title={`来源 session #${c.source_session_id!.slice(0, 8)} 已删除，需重绑或归档`}
                  >
                    ⚠ 来源缺失
                  </span>
                ) : (
                  <span className="eval-case-meta mono">
                    {c.source_session_id ? `#${c.source_session_id.slice(0, 8)}` : '—'}
                    {c.commit_sha ? ` · ${c.commit_sha.slice(0, 7)}` : ''}
                  </span>
                )}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

// ── P2 Case 详情 / 编辑 ──
function CaseDetail({
  caseId,
  cases,
  onChanged,
}: {
  caseId: string | null;
  cases: EvalCaseRow[];
  onChanged: () => Promise<void>;
}) {
  // 本地持有可编辑副本；caseId 变化时从列表快照初始化（避免每次 keystroke 拉库）。
  const initial = cases.find((c) => c.id === caseId) ?? null;
  const [draft, setDraft] = useState<EvalCaseRow | null>(initial);
  const [saving, setSaving] = useState(false);
  const [msg, setMsg] = useState<string | null>(null);

  useEffect(() => {
    setDraft(cases.find((c) => c.id === caseId) ?? null);
    setMsg(null);
  }, [caseId, cases]);

  if (!caseId) return <p className="eval-empty">从 P1 选一条 case 查看详情。</p>;
  if (!draft) return <p className="eval-empty">未找到 case（可能已被删除）。</p>;

  function set<K extends keyof EvalCaseRow>(key: K, val: EvalCaseRow[K]) {
    setDraft((d) => (d ? { ...d, [key]: val } : d));
  }

  async function save() {
    if (!draft) return;
    // 校验门：预期步骤 / 反例必须是 JSON 数组（score_eval_rubric 的 parse_names
    // 按数组消费；非数组会静默打错分）。坏契约直接拦下，不静默保存。
    const errs: string[] = [];
    if (!isArrayJson(draft.expected_steps_json)) errs.push('预期步骤须为 JSON 数组');
    if (!isArrayJson(draft.negative_json)) errs.push('反例须为 JSON 数组');
    if (errs.length > 0) {
      setMsg(`校验失败：${errs.join('；')}`);
      return;
    }
    setSaving(true);
    setMsg(null);
    try {
      const touched = await evalApi.updateCase(draft.id, {
        name: draft.name,
        category: draft.category,
        expectedStepsJson: draft.expected_steps_json ?? undefined,
        expectedOutput: draft.expected_output ?? undefined,
        expectedObservablesJson: draft.expected_observables_json ?? undefined,
        negativeJson: draft.negative_json ?? undefined,
      });
      await onChanged();
      setMsg(touched === 0 ? '未更新（case 不存在）' : '已保存契约字段');
    } catch (e) {
      setMsg(`保存失败：${e}`);
    } finally {
      setSaving(false);
    }
  }

  async function approve() {
    if (!draft) return;
    setSaving(true);
    setMsg(null);
    try {
      await evalApi.approveCase(draft.id);
      await onChanged();
      setMsg('已审核（draft→ready，可 anchor 配对）');
    } catch (e) {
      setMsg(`审核失败：${e}`);
    } finally {
      setSaving(false);
    }
  }

  return (
    <div className="eval-card">
      <div className="eval-section-label">输入 prompt（🔒 只读 · C1 对话记录不可改）</div>
      <div className="eval-locked mono">{draft.input_prompt}</div>

      <div className="eval-section-label">名称 / 分类</div>
      <div className="eval-row2">
        <input className="eval-input" value={draft.name} onChange={(e) => set('name', e.target.value)} />
        <input className="eval-input" value={draft.category} onChange={(e) => set('category', e.target.value)} />
      </div>

      <div className="eval-section-label">
        预期步骤序列（✏ 可编辑 · JSON，来自 extract_trajectory）
      </div>
      <textarea
        className="eval-textarea mono"
        rows={4}
        value={draft.expected_steps_json ?? ''}
        onChange={(e) => set('expected_steps_json', e.target.value || null)}
        placeholder='[{"name":"Read"},{"name":"Edit"}]'
      />
      <div className="eval-section-label">当前解析步骤（结构化预览 · 顺序敏感）</div>
      <div className="eval-step-list">
        {(() => {
          const steps = parseSteps(draft.expected_steps_json);
          return steps.length === 0 ? (
            <span className="eval-empty">（空 · 任何轨迹都 vacuous pass）</span>
          ) : (
            steps.map((s, i) => (
              <div key={i} className="eval-step">
                <span className="eval-step-idx">{i + 1}</span>
                <span className={`eval-tool-tag ${s.status === 'error' ? 'fail' : ''}`}>{s.name}</span>
              </div>
            ))
          );
        })()}
      </div>

      <div className="eval-section-label">expected observables（✏ 可编辑 · 怎么被证伪）</div>
      <textarea
        className="eval-textarea"
        rows={3}
        value={draft.expected_observables_json ?? ''}
        onChange={(e) => set('expected_observables_json', e.target.value || null)}
      />

      <div className="eval-section-label">expected output（✏ LLM 判的唯一字段）</div>
      <textarea
        className="eval-textarea"
        rows={2}
        value={draft.expected_output ?? ''}
        onChange={(e) => set('expected_output', e.target.value || null)}
      />

      <div className="eval-section-label">反例 negative（✏ 不该发生的步骤）</div>
      <textarea
        className="eval-textarea mono"
        rows={2}
        value={draft.negative_json ?? ''}
        onChange={(e) => set('negative_json', e.target.value || null)}
        placeholder='[{"name":"Bash"}]'
      />

      <div className="eval-actions">
        <Button variant="primary" onClick={save} disabled={saving}>
          {saving ? '保存中…' : '保存契约'}
        </Button>
        {draft.draft && (
          <Button variant="ghost" onClick={approve} disabled={saving}>
            审核为 ready
          </Button>
        )}
        {msg && <span className="eval-hint">{msg}</span>}
      </div>
    </div>
  );
}

// ── P3 会话 → Case ──
function SessionToCase({
  onChanged,
  onDone,
}: {
  onChanged: () => Promise<void>;
  onDone: () => void;
}) {
  const sessions = useAgentStore((s) => s.sessions);
  const finished = sessions.filter((s) => s.status === 'completed' || s.status === 'failed');

  const [sessionId, setSessionId] = useState('');
  const [traj, setTraj] = useState<FullTrajectory | null>(null);
  const [loading, setLoading] = useState(false);
  const [name, setName] = useState('');
  const [observables, setObservables] = useState('');
  const [negative, setNegative] = useState('');
  const [msg, setMsg] = useState<string | null>(null);

  useEffect(() => {
    if (!sessionId && finished.length > 0) setSessionId(finished[0].id);
  }, [finished, sessionId]);

  async function preview() {
    if (!sessionId) return;
    setLoading(true);
    setTraj(null);
    setMsg(null);
    try {
      const t = await evalApi.previewTrajectory(sessionId);
      setTraj(t);
      if (!name) {
        const sess = finished.find((x) => x.id === sessionId);
        if (sess) setName(sess.prompt.slice(0, 40) || '新 case');
      }
    } catch (e) {
      setMsg(`提取失败：${e}`);
    } finally {
      setLoading(false);
    }
  }

  async function save(draft: boolean) {
    if (!sessionId || !traj) return;
    setMsg(null);
    try {
      const sess = finished.find((x) => x.id === sessionId);
      const input: CreateCaseInput = {
        name: name || '未命名 case',
        category: 'agent',
        inputPrompt: sess?.prompt ?? '(无提示词)',
        // Freeze the OBJECTIVE extracted steps — what a future replay scores
        // against (反刷分 #1: contract from trace data, not the agent's say-so).
        expectedStepsJson: JSON.stringify(traj.steps),
        expectedObservablesJson: observables || undefined,
        negativeJson: negative || undefined,
        sourceSessionId: sessionId,
        draft,
      };
      const id = await evalApi.createCase(input);
      await onChanged();
      setMsg(`已入库（${draft ? 'probe 草稿' : 'ready'}）id=${id.slice(0, 8)}`);
      setTimeout(onDone, 600);
    } catch (e) {
      setMsg(`入库失败：${e}`);
    }
  }

  const failedN = traj ? traj.steps.filter((s) => s.status === 'error').length : 0;

  return (
    <div className="eval-card">
      <div className="eval-section-label">① 选会话</div>
      <select className="eval-input" value={sessionId} onChange={(e) => setSessionId(e.target.value)}>
        {finished.length === 0 && <option value="">暂无已完成会话</option>}
        {finished.map((s) => (
          <option key={s.id} value={s.id}>
            #{s.id.slice(0, 8)} · {s.prompt.slice(0, 40) || '(无提示词)'} {s.status === 'failed' ? '· 失败' : '· ✓'}
          </option>
        ))}
      </select>

      <div className="eval-section-label">② 自动提取轨迹（确定性，代码判）</div>
      <Button variant="ghost" onClick={preview} disabled={loading || !sessionId}>
        {loading ? '提取中…' : '提取轨迹'}
      </Button>
      {traj && (
        <div className="eval-steps-box">
          {/* Rich summary: steps / files / tokens / cost — all derived from the
              same trace rows + the session's recorded file diff, no LLM. */}
          <div className="eval-hint mono">
            提取到 {traj.steps.length} 步轨迹
            {failedN > 0 && <span className="eval-hint-err"> · ⚠ {failedN} 步失败</span>} · {traj.files_changed.length} 文件 ·{' '}
            {traj.input_tokens}+{traj.output_tokens} tokens · ≈ {traj.cost_cents.toFixed(3)}¢（估算）·{' '}
            {traj.span_tree.roots.length} LLM span
          </div>
          {(() => {
            // P3 边缘态：提取出的轨迹本身可能不可锚——空轨迹（vacuous pass）、
            // 全步失败（会话损坏）、异常高 token（疑似刷分）。显式标出来，让用户
            // 决定是否仍要以此轨迹冻结契约（反刷分：契约先要诚实）。
            const edges: string[] = [];
            if (traj.steps.length === 0) edges.push('空轨迹（纯文本轮 · vacuous pass）');
            else if (failedN === traj.steps.length) edges.push('全步骤失败（会话损坏？）');
            const totTok = traj.input_tokens + traj.output_tokens;
            if (totTok > 200000)
              edges.push(`token 异常高（${Math.round(totTok / 1000)}k · 疑似刷分）`);
            if (edges.length === 0) return null;
            return (
              <div className="eval-edge-row">
                {edges.map((e, i) => (
                  <span key={i} className="eval-badge eval-badge-unclear">
                    ⚠ {e}
                  </span>
                ))}
              </div>
            );
          })()}
          <div className="eval-step-list">
            {traj.steps.length === 0 ? (
              <span className="eval-empty">该会话无工具调用（纯文本轮）</span>
            ) : (
              traj.steps.map((s, i) => (
                <div key={i} className="eval-step">
                  <span className="eval-step-idx">{i + 1}</span>
                  <span className={`eval-tool-tag ${s.status === 'error' ? 'fail' : ''}`}>{s.name}</span>
                </div>
              ))
            )}
          </div>
          {traj.files_changed.length > 0 && (
            <div className="eval-files-box">
              <div className="eval-section-label">文件变更（git diff 快照 + per-write）</div>
              <div className="eval-file-list mono">
                {traj.files_changed.map((f) => (
                  <span key={f} className="eval-file-tag">
                    {f}
                  </span>
                ))}
              </div>
            </div>
          )}
        </div>
      )}

      <div className="eval-section-label">③ 名称 / observables（人工）</div>
      <input className="eval-input" value={name} onChange={(e) => setName(e.target.value)} placeholder="case 名称" />
      <textarea
        className="eval-textarea"
        rows={2}
        value={observables}
        onChange={(e) => setObservables(e.target.value)}
        placeholder="pill 颜色随 tool_result 变；不改 ToolResultPill"
      />
      <textarea
        className="eval-textarea mono"
        rows={2}
        value={negative}
        onChange={(e) => setNegative(e.target.value)}
        placeholder='反例 [{"name":"Bash"}]'
      />

      <div className="eval-actions">
        <Button variant="primary" onClick={() => save(false)} disabled={!traj}>
          入库为 ready
        </Button>
        <Button variant="ghost" onClick={() => save(true)} disabled={!traj}>
          存为 probe
        </Button>
        {msg && <span className="eval-hint">{msg}</span>}
      </div>
    </div>
  );
}

// ── P4 评测任务配置（3 类评测对象）──
type EvalObject = 'agent' | 'platform-e2e' | 'platform-enablement';

function ReplayLaunch({
  cases,
  onReplayed,
  pickCase,
}: {
  cases: EvalCaseRow[];
  onReplayed: () => Promise<void>;
  pickCase: (id: string) => void;
}) {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const sessions = useAgentStore((s) => s.sessions);
  const workingFallback = activeProject?.path ?? sessions[0]?.projectPath ?? '';

  const [obj, setObj] = useState<EvalObject>('agent');
  const readyCases = cases.filter((c) => !c.draft);
  const [caseId, setCaseId] = useState('');
  const [workingDir, setWorkingDir] = useState(workingFallback);
  const [matcher, setMatcher] = useState<Matcher>('exact_match');
  const [model, setModel] = useState('');
  const [enableSkills, setEnableSkills] = useState(true);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<ReplayVerdict | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    if (!caseId && readyCases.length > 0) setCaseId(readyCases[0].id);
  }, [readyCases, caseId]);

  useEffect(() => {
    setWorkingDir(workingFallback);
  }, [workingFallback]);

  async function run() {
    if (!caseId || !workingDir) return;
    setRunning(true);
    setErr(null);
    setResult(null);
    try {
      const v = await evalApi.runReplay(
        caseId,
        workingDir,
        matcher,
        model.trim() || undefined,
        enableSkills,
      );
      setResult(v);
      await onReplayed();
    } catch (e) {
      setErr(String(e));
    } finally {
      setRunning(false);
    }
  }

  return (
    <div className="eval-card">
      <div className="eval-section-label">① 评测对象（不只评测 agent）</div>
      <div className="eval-object-grid">
        {(
          [
            ['agent', 'Agent', 'coding 任务 → agent 工具/可靠性 · 轻量'],
            ['platform-e2e', '平台-e2e', 'IPC+前端+数据流 · 全栈'],
            ['platform-enablement', '平台-加持', '开/关 DW 功能 → agent 增量'],
          ] as [EvalObject, string, string][]
        ).map(([id, label, hint]) => (
          <label key={id} className={`eval-object ${obj === id ? 'active' : ''}`}>
            <input type="radio" name="obj" checked={obj === id} onChange={() => setObj(id)} />
            <b>{label}</b>
            <div className="eval-hint">{hint}</div>
          </label>
        ))}
      </div>
      {obj === 'platform-e2e' && <PlatformE2eRunner />}
      {obj === 'platform-enablement' && <PlatformEnablementRunner cases={cases} />}

      {obj === 'agent' && (
        <>
          <div className="eval-section-label">② 功能开关矩阵 + 环境（模型）</div>
          <div className="eval-feature-matrix">
            <label className={`eval-feature-toggle ${enableSkills ? 'on' : 'off'}`}>
              <input
                type="checkbox"
                checked={enableSkills}
                onChange={(e) => setEnableSkills(e.target.checked)}
              />
              <b>Skills</b>
              <span className="eval-hint">真生效 · enable_skills</span>
            </label>
            {(
              [
                ['GateBar', '熔断'],
                ['Harness', '测试 harness'],
                ['compaction', '上下文压缩'],
              ] as [string, string][]
            ).map(([k, desc]) => (
              <div key={k} className="eval-feature-toggle off fixed">
                <b>{k}</b>
                <span className="eval-hint">{desc} · replay 单轮只读沙箱不注入</span>
              </div>
            ))}
          </div>
          <input
            className="eval-input"
            value={model}
            onChange={(e) => setModel(e.target.value)}
            placeholder="模型 id（留空=默认；glm-4.6 / deepseek-chat / kimi …）"
          />
          <div className="eval-hint">
            环境控制：模型切换（大窗口 Kimi/DeepSeek 与 GLM 的窗口差异由 provider 配置处理）。GateBar/Harness/compaction
            在 replay 单轮只读沙箱中本就不注入——矩阵如实标注，不造假开关。
          </div>

          <div className="eval-section-label">③ Case（已审核）</div>
          <div className="eval-case-list">
            {readyCases.length === 0 ? (
              <span className="eval-empty">无已审核 case。先在 P2 审核。</span>
            ) : (
              readyCases.map((c) => (
                <button
                  key={c.id}
                  className={`eval-case-row ${caseId === c.id ? 'selected' : ''}`}
                  onClick={() => setCaseId(c.id)}
                >
                  <span className="eval-badge eval-badge-clear">ready</span>
                  <span className="eval-case-title">{c.name}</span>
                  <span className="eval-hint mono">{c.category}</span>
                </button>
              ))
            )}
          </div>

          <div className="eval-section-label">④ 工作区 / 匹配器</div>
          <div className="eval-row2">
            <input className="eval-input" value={workingDir} onChange={(e) => setWorkingDir(e.target.value)} placeholder="工作区路径（只读沙箱）" />
            <select className="eval-input" value={matcher} onChange={(e) => setMatcher(e.target.value as Matcher)}>
              {MATCHERS.map((m) => (
                <option key={m} value={m}>
                  {MATCHER_LABEL[m]}
                </option>
              ))}
            </select>
          </div>
          <div className="eval-hint">
            Plan 沙箱：agent 只能 Read/Glob/Grep，Bash/Write 在 hook 层被拦。轨迹是工具选择，非执行副作用。
          </div>

          <div className="eval-actions">
            <Button variant="primary" onClick={run} disabled={running || !caseId || !workingDir}>
              {running ? '回放中…' : '▶ 运行回放'}
            </Button>
            {result && (
              <span className="eval-replay-result">
                <VerdictBadge verdict={result.verdict} />
                <span className="mono">
                  {' '}
                  {result.score.toFixed(2)} · {result.negative_violated ? '⚠ 反例命中' : '无反例'}
                </span>
              </span>
            )}
            {err && <span className="eval-hint eval-hint-err">{err}</span>}
          </div>
          {result && (
            <div className="eval-locked mono">
              {result.reason}
              <br />
              查看完整 verdict →{' '}
              <button className="eval-link" onClick={() => pickCase(caseId)}>
                P2 / V1
              </button>
            </div>
          )}
        </>
      )}
    </div>
  );
}

// ── P4 平台-e2e eval 运行器（数据平面：临时内存库 + 真 schema + 真逻辑）──
const SAMPLE_E2E_SEED = `{
  "cases": [
    { "id": "c1", "name": "demo", "category": "agent", "input_prompt": "do",
      "expected_steps_json": "[{\\"name\\":\\"read\\"},{\\"name\\":\\"edit\\"}]",
      "negative_json": "[{\\"name\\":\\"bash\\"}]", "draft": false },
    { "id": "c2", "name": "draft-demo", "category": "agent", "input_prompt": "x", "draft": true }
  ],
  "verdicts": [
    { "gate": "eval", "verdict": "PASS", "case_id": "c1" }
  ]
}`;
const SAMPLE_E2E_EXPECT = `{
  "approved_case_count": 1,
  "total_case_count": 2,
  "verdict_count_for_gate": ["eval", 1],
  "replay": { "case_id": "c1", "actual_steps": ["read", "edit"], "expected_grade": "optimal" }
}`;

function PlatformE2eRunner() {
  const [seedText, setSeedText] = useState(SAMPLE_E2E_SEED);
  const [expectText, setExpectText] = useState(SAMPLE_E2E_EXPECT);
  const [running, setRunning] = useState(false);
  const [verdict, setVerdict] = useState<E2EVerdict | null>(null);
  const [err, setErr] = useState<string | null>(null);

  async function run() {
    setRunning(true);
    setErr(null);
    setVerdict(null);
    try {
      const seed = JSON.parse(seedText) as E2ESeed;
      const expect = JSON.parse(expectText) as E2EExpect;
      const v = await evalApi.runPlatformE2e(seed, expect);
      setVerdict(v);
    } catch (e) {
      setErr(String(e));
    } finally {
      setRunning(false);
    }
  }

  return (
    <div className="eval-mechanism">
      <div className="eval-section-label">Seed（灌入临时库的 cases + verdicts）</div>
      <textarea
        className="eval-yaml mono"
        value={seedText}
        onChange={(e) => setSeedText(e.target.value)}
        rows={8}
        spellCheck={false}
      />
      <div className="eval-section-label">期望（确定性契约）</div>
      <textarea
        className="eval-yaml mono"
        value={expectText}
        onChange={(e) => setExpectText(e.target.value)}
        rows={8}
        spellCheck={false}
      />
      <div className="eval-hint">
        临时内存库（真 schema）→ seed → 对真 list_eval_cases / score_replay / list_verdicts 断言。
        无 LLM/无浏览器，判决=数据契约客观事实；渲染层由 playwright eval.spec.ts 守护。
      </div>
      <div className="eval-actions">
        <Button variant="primary" onClick={run} disabled={running}>
          {running ? '驱动数据平面…' : '▶ 运行 e2e 评测'}
        </Button>
        {verdict && (
          <span className="eval-replay-result">
            <VerdictBadge verdict={verdict.pass ? 'PASS' : 'FAIL'} />
            <span className="mono"> {verdict.checks.length} 项检查</span>
          </span>
        )}
        {err && <span className="eval-hint eval-hint-err">{err}</span>}
      </div>
      {verdict && (
        <div className="eval-locked mono">
          {verdict.checks.map((c, i) => (
            <div key={i}>
              {c.pass ? '✓' : '✗'} {c.name} — {c.detail}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ── P4 平台-加持 eval 运行器（开/关 skills → 配对 diff，需 live key）──
function PlatformEnablementRunner({ cases }: { cases: EvalCaseRow[] }) {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const sessions = useAgentStore((s) => s.sessions);
  const workingFallback = activeProject?.path ?? sessions[0]?.projectPath ?? '';
  const readyCases = cases.filter((c) => !c.draft);
  const [caseId, setCaseId] = useState('');
  const [workingDir, setWorkingDir] = useState(workingFallback);
  const [running, setRunning] = useState(false);
  const [verdict, setVerdict] = useState<EnablementVerdict | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    if (!caseId && readyCases.length > 0) setCaseId(readyCases[0].id);
  }, [readyCases, caseId]);
  useEffect(() => {
    setWorkingDir(workingFallback);
  }, [workingFallback]);

  async function run() {
    if (!caseId || !workingDir) return;
    setRunning(true);
    setErr(null);
    setVerdict(null);
    try {
      const v = await evalApi.runEnablement(caseId, workingDir, 'exact_match');
      setVerdict(v);
    } catch (e) {
      setErr(String(e));
    } finally {
      setRunning(false);
    }
  }

  return (
    <div className="eval-mechanism">
      <div className="eval-section-label">被测 case + 工作区</div>
      <div className="eval-case-list">
        {readyCases.length === 0 ? (
          <span className="eval-empty">无已审核 case。先在 P2 审核。</span>
        ) : (
          readyCases.map((c) => (
            <button
              key={c.id}
              className={`eval-case-row ${caseId === c.id ? 'selected' : ''}`}
              onClick={() => setCaseId(c.id)}
            >
              <span className="eval-badge eval-badge-clear">ready</span>
              <span className="eval-case-title">{c.name}</span>
            </button>
          ))
        )}
      </div>
      <input
        className="eval-input"
        value={workingDir}
        onChange={(e) => setWorkingDir(e.target.value)}
        placeholder="工作区路径（只读沙箱）"
      />
      <div className="eval-hint">
        skills 关→开两次真 agent 回放（Plan 沙箱），compare_paired diff 轨迹。
        <b>需 live provider key。</b>反刷分：增益须闭合到 expected 缺口才算 CLEAR，否则 BRAKE；无 delta 则 NoChange。
      </div>
      <div className="eval-actions">
        <Button variant="primary" onClick={run} disabled={running || !caseId || !workingDir}>
          {running ? '两次回放 diff…' : '▶ 运行加持评测'}
        </Button>
        {verdict && (
          <span className="eval-replay-result">
            {verdict.attribution && <VerdictBadge verdict={verdict.attribution} />}
            <span className="mono">
              {' '}
              {verdict.outcome} · off {verdict.off_score.toFixed(2)} → on {verdict.on_score.toFixed(2)}
            </span>
          </span>
        )}
        {err && <span className="eval-hint eval-hint-err">{err}</span>}
      </div>
      {verdict && <div className="eval-locked mono">{verdict.reason}</div>}
    </div>
  );
}

// ── P5 Paired 对比 ──
function PairedCompare({
  cases,
  verdicts,
}: {
  cases: EvalCaseRow[];
  verdicts: VerdictRow[];
}) {
  const readyCases = cases.filter((c) => !c.draft);
  const [caseId, setCaseId] = useState('__all__');

  // 单 case 视图：取该 case 的 eval verdicts 新旧各一（list_verdicts new-first：
  // [0]=新, [1]=旧）。'__all__' 视图：聚合每个 ready case 的净判定，按反刷分保守序
  // 判定——split（提升+回归并存）=分歧待人审（不简单放/拦）/ 纯 regress=拦 /
  // 全 same（无提升无回归）=持平待判 / 否则全 improve=净提升可准入。
  const caseOptions = (
    <>
      <option value="__all__">全部 case（聚合准入判定）</option>
      {readyCases.map((c) => (
        <option key={c.id} value={c.id}>
          {c.name}
        </option>
      ))}
    </>
  );

  if (caseId === '__all__') {
    const perCase = readyCases
      .map((c) => {
        const evs = verdicts.filter((v) => v.gate === 'eval' && v.case_id === c.id);
        if (evs.length < 2) return { c, net: null as 'improve' | 'regress' | 'same' | null, n: evs.length };
        return { c, net: netVerdict(evs[1], evs[0]), n: evs.length };
      });
    const judged = perCase.filter((x) => x.net !== null);
    const improves = judged.filter((x) => x.net === 'improve').length;
    const regresses = judged.filter((x) => x.net === 'regress').length;
    const split = improves > 0 && regresses > 0;
    return (
      <div className="eval-card">
        <div className="eval-section-label">选 Case（按 case_id 取其 eval verdicts，新旧各一）</div>
        <select className="eval-input" value={caseId} onChange={(e) => setCaseId(e.target.value)}>
          {caseOptions}
        </select>
        {judged.length === 0 ? (
          <p className="eval-empty">无 case 有 ≥2 次 eval verdict。至少回放两次（P4）才能配对。</p>
        ) : (
          <div className="eval-compare-summary">
            {split ? (
              <span className="eval-badge eval-badge-unclear">
                分歧 · 待判（{improves} 提升 / {regresses} 回归 / {judged.length - improves - regresses} 持平，人审）
              </span>
            ) : regresses > 0 ? (
              <span className="eval-badge eval-badge-brake">回归 · 拦（{regresses}/{judged.length} case 倒退）</span>
            ) : improves === 0 ? (
              <span className="eval-badge eval-badge-muted">持平 · 待判（0/{judged.length} case 无变化）</span>
            ) : (
              <span className="eval-badge eval-badge-clear">净提升 · 可准入（{improves}/{judged.length}）</span>
            )}
          </div>
        )}
        <div className="eval-case-list">
          {perCase.map(({ c, net, n }) => (
            <button key={c.id} className="eval-case-row" onClick={() => setCaseId(c.id)}>
              <span className="eval-case-title">{c.name}</span>
              <span className="eval-hint mono">{n} 次</span>
              {net === 'improve' && <span className="eval-badge eval-badge-clear">提升</span>}
              {net === 'regress' && <span className="eval-badge eval-badge-brake">回归</span>}
              {net === 'same' && <span className="eval-badge eval-badge-muted">持平</span>}
              {net === null && <span className="eval-badge eval-badge-muted">不足2次</span>}
            </button>
          ))}
        </div>
      </div>
    );
  }

  const evalVerdicts = verdicts.filter((v) => v.gate === 'eval' && v.case_id === caseId);
  // list_verdicts 是 new-first：[0]=新(本次), [1]=旧(上次)。解构名须与序对齐——
  // 旧版 `[oldV, newV]` 把新赋给 oldV，导致 newScore(old,new) 反号、提升被误判
  // 回归（配对回放的准入/刹车判定反转）。按名赋值：newV=[0], oldV=[1]。
  const [newV, oldV] = evalVerdicts;

  return (
    <div className="eval-card">
      <div className="eval-section-label">选 Case（按 case_id 取其 eval verdicts，新旧各一）</div>
      <select className="eval-input" value={caseId} onChange={(e) => setCaseId(e.target.value)}>
        {caseOptions}
      </select>

      {evalVerdicts.length < 2 ? (
        <p className="eval-empty">
          该 case 的 eval verdict 不足 2 条（{evalVerdicts.length}）。至少回放两次（P4）才能配对对比。
        </p>
      ) : (
        <div className="eval-compare">
          <CompareCol title={`旧 · ${fmtTime(oldV.created_at)}`} v={oldV} kind="old" otherSteps={stepsOf(newV)} />
          <CompareCol
            title={`新 · ${fmtTime(newV.created_at)}`}
            v={newV}
            kind={newScore(oldV, newV)}
            otherSteps={stepsOf(oldV)}
          />
          <div className="eval-compare-summary">
            {netVerdict(oldV, newV) === 'improve' ? (
              <span className="eval-badge eval-badge-clear">净提升 · 可准入</span>
            ) : netVerdict(oldV, newV) === 'regress' ? (
              <span className="eval-badge eval-badge-brake">回归 · 拦</span>
            ) : (
              <span className="eval-badge eval-badge-unclear">持平 · 待判</span>
            )}
            <span className="eval-hint mono">
              {' '}
              旧 {oldV.verdict}({scoreOf(oldV).toFixed(2)}) → 新 {newV.verdict}({scoreOf(newV).toFixed(2)})
            </span>
          </div>
        </div>
      )}
    </div>
  );
}

export function scoreOf(v: VerdictRow): number {
  try {
    const r = JSON.parse(v.report ?? '{}') as { score?: number };
    if (typeof r.score === 'number') return r.score;
    // VerdictBadge treats a leading-digit verdict ("0.85") as a pass — mirror
    // that here so a numeric-verdict row scores as its value, not 0. Without
    // this, a "0.85" verdict would badge green but score 0 in PairedCompare,
    // silently flipping a CLEAR/BRAKE (反刷分: mis-scored verdict).
    if (/^\d/.test(v.verdict)) return parseFloat(v.verdict) || 0;
    return v.verdict === 'PASS' ? 1 : 0;
  } catch {
    return 0;
  }
}

/// The actual tool-step names a verdict's replay ran (from its report). Empty
/// when the report carries none. P5's paired compare diffs old vs new on this.
function stepsOf(v: VerdictRow): string[] {
  try {
    const r = JSON.parse(v.report ?? '{}') as { actual_steps?: string[] };
    return r.actual_steps ?? [];
  } catch {
    return [];
  }
}

function newScore(oldV: VerdictRow, newV: VerdictRow): 'improve' | 'regress' | 'same' {
  const d = scoreOf(newV) - scoreOf(oldV);
  if (d > 0.001) return 'improve';
  if (d < -0.001) return 'regress';
  return 'same';
}

function netVerdict(oldV: VerdictRow, newV: VerdictRow): 'improve' | 'regress' | 'same' {
  return newScore(oldV, newV);
}

function CompareCol({
  title,
  v,
  kind,
  otherSteps,
}: {
  title: string;
  v: VerdictRow;
  kind: string;
  otherSteps: string[];
}) {
  const steps = stepsOf(v);
  // Diff highlight: a step UNIQUE to this side (absent from the paired other
  // side, case-insensitive) is what the strategy change added/removed — the
  // anti-gaming signal in 配对回放. Tag it so the eye lands on the delta, not
  // the shared prefix.
  const other = new Set(otherSteps.map((s) => s.toLowerCase()));
  return (
    <div className={`eval-compare-col ${kind === 'regress' ? 'regress' : kind === 'improve' ? 'improve' : ''}`}>
      <div className="eval-col-head">
        <span>{title}</span>
        <VerdictBadge verdict={v.verdict} />
      </div>
      <div className="eval-step-list">
        {steps.length === 0 ? (
          <span className="eval-hint mono">（无轨迹步骤）</span>
        ) : (
          steps.map((s, i) => {
            const unique = !other.has(s.toLowerCase());
            return (
              <div key={i} className="eval-step">
                <span className="eval-step-idx">{i + 1}</span>
                <span className={`eval-tool-tag ${unique ? (kind === 'regress' ? 'fail' : 'improve') : ''}`}>{s}</span>
              </div>
            );
          })
        )}
      </div>
      <AttributionBadge value={v.attribution} />
    </div>
  );
}

// ── P6 rubric（8 维 AgentX 可靠性）──
function RubricCard({
  verdicts,
  caseName,
}: {
  verdicts: VerdictRow[];
  caseName: (id: string | null) => string;
}) {
  const evalVs = verdicts.filter((v) => v.gate === 'eval');
  const latest = evalVs[0];

  const [score, setScore] = useState<RubricScore | null>(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  // Fetch the rubric for the latest eval verdict's session×case. The verdict
  // carries the replay's session_id (run_eval_replay mints a fresh one) +
  // case_id — exactly what score_eval_rubric assembles RubricInput from.
  useEffect(() => {
    if (!latest?.session_id || !latest?.case_id) {
      setScore(null);
      setErr(null);
      return;
    }
    let cancelled = false;
    setLoading(true);
    setErr(null);
    evalApi
      .scoreRubric(latest.session_id!, latest.case_id!)
      .then((s) => {
        if (!cancelled) setScore(s);
      })
      .catch((e) => {
        if (!cancelled) setErr(String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [latest?.session_id, latest?.case_id]);

  if (!latest) {
    return <p className="eval-empty">尚无 eval verdict。先在 P4 跑一次回放。</p>;
  }
  if (!latest.session_id || !latest.case_id) {
    return (
      <div className="eval-card">
        <div className="eval-hint mono">
          最近 eval verdict · {verdictTarget(latest, caseName)} · {fmtTime(latest.created_at)}
        </div>
        <div className="eval-gap-note">
          ⚠ 该 verdict 缺 session_id / case_id（Executor gate 跨 crate trait 无 session 上下文，verdict
          先存 None）。8 维 rubric 需 session×case 才能装配 —— 去 P4 跑一次回放（run_eval_replay 自带
          session_id）即可在此看到。
        </div>
      </div>
    );
  }

  return (
    <div className="eval-card">
      <div className="eval-hint mono">
        最近 eval verdict · {verdictTarget(latest, caseName)} · {fmtTime(latest.created_at)} · session #
        {latest.session_id.slice(0, 8)}
      </div>

      {loading && <p className="eval-empty">计算 8 维 rubric…</p>}
      {err && <p className="eval-empty">计算失败：{err}</p>}

      {score && (
        <>
          <div className="eval-rubric-q">
            <span className="eval-rubric-q-label">Q_code（加权可靠性）</span>
            <span className={`eval-rubric-q-val ${score.q_code > 0.7 ? 'good' : score.q_code > 0.4 ? 'warn' : 'bad'}`}>
              {score.q_code.toFixed(3)}
            </span>
            {score.hard_gate_triggered && (
              <span className="eval-badge eval-badge-brake">⚠ 硬门触发 · Q=0</span>
            )}
          </div>
          <div className="eval-rubric">
            {score.dims.map((d) => (
              <RubricRow key={d.key} name={d.label} pct={d.score * 100} val={d.val} hard={d.hard} />
            ))}
          </div>
          <div className="eval-hint">
            8 维全部确定性派生自 trace/文件/契约（反刷分 #1：LLM 不给自己的可靠性打分）。manual
            intervention 是硬门：任何人工干预→Q 归零。replay 跑 Plan 只读沙箱，本无人工干预。
          </div>
        </>
      )}
    </div>
  );
}

function RubricRow({ name, pct, val, hard }: { name: string; pct: number; val: string; hard?: boolean }) {
  return (
    <div className={`eval-rubric-row ${hard ? 'hard' : ''}`}>
      <div className="eval-rubric-name">{name}</div>
      <div className="eval-rubric-bar">
        <i className={pct < 50 ? 'low' : ''} style={{ width: `${Math.max(0, Math.min(100, pct))}%` }} />
      </div>
      <div className="eval-rubric-val mono">{val}</div>
    </div>
  );
}

// ── V1 Verdicts ──
function VerdictsLedger({
  verdicts,
  caseName,
}: {
  verdicts: VerdictRow[];
  caseName: (id: string | null) => string;
}) {
  const [gate, setGate] = useState('all');
  const [attr, setAttr] = useState('all');
  const [q, setQ] = useState('');

  const rows = verdicts.filter((v) => {
    if (gate !== 'all' && v.gate !== gate) return false;
    if (attr !== 'all' && v.attribution !== attr) return false;
    if (!q) return true;
    const hay = `${verdictTarget(v, caseName)} ${v.gate} ${v.verdict} ${v.session_id ?? ''} ${v.case_id ?? ''}`.toLowerCase();
    return hay.includes(q.toLowerCase());
  });

  return (
    <div className="eval-card">
      <div className="eval-toolbar">
        <select className="eval-input" value={gate} onChange={(e) => setGate(e.target.value)}>
          {['all', 'verify', 'honesty', 'forge', 'circuit-breaker', 'eval'].map((g) => (
            <option key={g} value={g}>
              {g === 'all' ? '全部 gate' : g}
            </option>
          ))}
        </select>
        <select className="eval-input" value={attr} onChange={(e) => setAttr(e.target.value)}>
          {['all', 'CLEAR', 'UNCLEAR', 'BRAKE'].map((a) => (
            <option key={a} value={a}>
              {a === 'all' ? '全部 attribution' : a}
            </option>
          ))}
        </select>
        <input className="eval-input" placeholder="case / session" value={q} onChange={(e) => setQ(e.target.value)} />
      </div>

      {rows.length === 0 ? (
        <p className="eval-empty">无匹配判决。</p>
      ) : (
        <div className="eval-verdict-table" data-testid="eval-verdict-table">
          <div className="eval-vrow eval-vrow-head mono">
            <span>gate</span>
            <span>目标</span>
            <span>attribution</span>
            <span>verdict</span>
            <span>时间</span>
          </div>
          {rows.map((v) => (
            <div key={v.id} className="eval-vrow">
              <GateBadge gate={v.gate} />
              <span className="eval-vrow-target">{verdictTarget(v, caseName)}</span>
              <AttributionBadge value={v.attribution} />
              <VerdictBadge verdict={v.verdict} />
              <span className="eval-hint mono">{fmtTime(v.created_at)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ── V2 失败资产知识库（FAIL/TRIPPED/BRAKE 派生 · 可搜索 / 多索引）──
function FailureAssets({
  verdicts,
  caseName,
}: {
  verdicts: VerdictRow[];
  caseName: (id: string | null) => string;
}) {
  const [q, setQ] = useState('');
  const [indexBy, setIndexBy] = useState<'gate' | 'attribution' | 'reason'>('gate');

  const fails = verdicts.filter(
    (v) => v.verdict === 'FAIL' || v.verdict === 'TRIPPED' || v.attribution === 'BRAKE',
  );

  // root cause：优先 eval report 的 reason（score_replay 写入的具体命中描述），
  // 否则回退 verdict 串。是「这条为什么挂」的一句话索引。
  const reasonOf = useCallback((v: VerdictRow): string => {
    try {
      const r = JSON.parse(v.report ?? '{}') as { reason?: string };
      return r.reason ?? v.verdict;
    } catch {
      return v.verdict;
    }
  }, []);

  const rows = fails.filter((v) => {
    if (!q) return true;
    const hay =
      `${verdictTarget(v, caseName)} ${v.gate} ${v.attribution ?? ''} ${reasonOf(v)} ${v.case_id ?? ''} ${v.session_id ?? ''}`.toLowerCase();
    return hay.includes(q.toLowerCase());
  });

  // 多索引：按 stage(gate) / lever(attribution) / root-cause(reason) 聚合，让用户
  // 从不同维度检索失败资产（反刷分：失败要能被下轮 case 生成前查到规避）。
  const groups = useMemo(() => {
    const m = new Map<string, VerdictRow[]>();
    for (const v of rows) {
      const key =
        indexBy === 'gate'
          ? v.gate
          : indexBy === 'attribution'
            ? v.attribution ?? 'UNCLEAR'
            : reasonOf(v).slice(0, 80);
      const arr = m.get(key) ?? [];
      arr.push(v);
      m.set(key, arr);
    }
    return [...m.entries()].sort((a, b) => b[1].length - a[1].length);
  }, [rows, indexBy, reasonOf]);

  return (
    <div className="eval-card">
      <div className="eval-hint">
        失败资产从 FAIL / TRIPPED / BRAKE 判决派生。可搜索 + 按 stage(gate) / lever(attribution) /
        root-cause(reason) 索引，下次 case 生成前检索规避。
      </div>
      {fails.length === 0 ? (
        <p className="eval-empty">尚无失败沉淀（无 FAIL/TRIPPED/BRAKE 判决）。</p>
      ) : (
        <>
          <div className="eval-toolbar">
            <input
              className="eval-input"
              placeholder="搜索 case / session / reason / gate"
              value={q}
              onChange={(e) => setQ(e.target.value)}
            />
            <select
              className="eval-input"
              value={indexBy}
              onChange={(e) => setIndexBy(e.target.value as typeof indexBy)}
            >
              <option value="gate">索引 · stage (gate)</option>
              <option value="attribution">索引 · lever (attribution)</option>
              <option value="reason">索引 · root cause (reason)</option>
            </select>
          </div>
          {groups.length === 0 ? (
            <p className="eval-empty">无匹配失败。</p>
          ) : (
            <div className="eval-fail-list">
              {groups.map(([key, vs]) => (
                <div key={key} className="eval-fail-card">
                  <div className="eval-fail-head">
                    <b>{key}</b>
                    <span className="eval-badge eval-badge-brake">×{vs.length}</span>
                  </div>
                  <div className="eval-hint mono">
                    {vs
                      .slice(0, 5)
                      .map((v) => verdictTarget(v, caseName))
                      .join(' · ')}
                  </div>
                </div>
              ))}
            </div>
          )}
        </>
      )}
    </div>
  );
}

// ── V3 模糊 FAIL：归因链 + reviewer 原文 + 人判按钮 ──

/// 前端镜像后端 gates.rs `parse_verdict`，把「为什么这条判 FAIL」拆成可读的归因链
/// 三步（① 首行 VERDICT 契约 ② body fail marker 覆盖 ③ keyword fallback / 默认 FAIL）。
/// 必须与后端逐字同形——这是反刷分账本卫生：UI 展示的归因要等于后端实际判定。
/// 后端改动（gates.rs:67）须同步此处。
export function parseVerdictTrace(report: string): { steps: { label: string; hit: boolean }[]; passed: boolean } {
  const hasFailMarker = (text: string) => {
    const l = text.toLowerCase();
    return l.includes('fail') || l.includes('defect') || l.includes('不通过') || l.includes('缺陷');
  };
  const firstLine = report.split('\n')[0] ?? '';
  // 与后端一致：trim + 去尾句末标点（.。!？!?）后小写精确匹配。
  const norm = firstLine.trim().replace(/[.。!？!?]+$/, '').toLowerCase();
  const steps: { label: string; hit: boolean }[] = [];

  if (norm === 'verdict: pass') {
    steps.push({ label: '① 首行 VERDICT: PASS 契约命中', hit: true });
    if (hasFailMarker(report)) {
      steps.push({
        label: '② body fail marker 覆盖 → FAIL（reviewer 正文含 fail/defect/不通过/缺陷）',
        hit: true,
      });
      return { steps, passed: false };
    }
    return { steps, passed: true };
  }
  if (norm === 'verdict: fail') {
    steps.push({ label: '① 首行 VERDICT: FAIL 契约命中 → 直接 FAIL', hit: true });
    return { steps, passed: false };
  }
  steps.push({
    label: `① 首行 VERDICT 契约未命中（违约/空）→ 降级 keyword`,
    hit: false,
  });

  const l = report.toLowerCase();
  const pass = l.includes('pass') || l.includes('通过');
  const failMarker = hasFailMarker(l);
  if (pass && !failMarker) {
    steps.push({ label: '③ keyword fallback → PASS（含 pass/通过，无 fail marker）', hit: true });
    return { steps, passed: true };
  }
  if (failMarker) {
    steps.push({ label: '③ keyword fallback → FAIL（fail marker 主导）', hit: true });
    return { steps, passed: false };
  }
  steps.push({ label: '③ 默认 FAIL（模糊/无可判信号 · 对抗性默认）', hit: true });
  return { steps, passed: false };
}

function AmbiguousFail({
  verdicts,
  caseName,
}: {
  verdicts: VerdictRow[];
  caseName: (id: string | null) => string;
}) {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const sessions = useAgentStore((s) => s.sessions);
  const workingFallback = activeProject?.path ?? sessions[0]?.projectPath ?? '';
  // 人判反馈：confirm（认同 FAIL）/ dispute（推翻）。本地态——后端持久化 verify-gate
  // 人判反馈是后续工作（与 human-gate verdict 同机制，但这条是 verify gate）。
  const [judged, setJudged] = useState<Record<string, 'confirm' | 'dispute'>>({});
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});
  const [rerunMsg, setRerunMsg] = useState<Record<string, string>>({});
  const [rerunning, setRerunning] = useState<Record<string, boolean>>({});

  // 模糊 FAIL = verify/honesty 的 FAIL，且 report 含「模糊/未含标准/默认/fallback」线索，
  // 或归因链落在 ③ 默认 FAIL（无可判信号）。
  const ambiguous = verdicts.filter((v) => {
    if (v.verdict !== 'FAIL' || !['verify', 'honesty'].includes(v.gate)) return false;
    const blob = `${v.report ?? ''}`.toLowerCase();
    const lexical = ['模糊', '未含标准', '默认', 'fallback', 'ambiguous'].some((k) => blob.includes(k));
    const trace = parseVerdictTrace(v.report ?? '');
    const defaultFail = trace.steps.some((s) => s.label.startsWith('③ 默认 FAIL'));
    return lexical || defaultFail;
  });

  async function doRerun(v: VerdictRow) {
    if (!v.case_id) {
      setRerunMsg((r) => ({ ...r, [v.id]: '该 verdict 无 case_id，无法重跑' }));
      return;
    }
    if (!workingFallback) {
      setRerunMsg((r) => ({ ...r, [v.id]: '需先选活动项目（工作区）才能重跑' }));
      return;
    }
    setRerunning((r) => ({ ...r, [v.id]: true }));
    setRerunMsg((r) => ({ ...r, [v.id]: '重跑中…' }));
    try {
      const res = await evalApi.runReplay(v.case_id, workingFallback, 'exact_match');
      setRerunMsg((r) => ({ ...r, [v.id]: `重跑完成：${res.verdict} (${res.score.toFixed(2)})` }));
    } catch (e) {
      setRerunMsg((r) => ({ ...r, [v.id]: `重跑失败：${e}` }));
    } finally {
      setRerunning((r) => ({ ...r, [v.id]: false }));
    }
  }

  return (
    <div className="eval-card">
      <div className="eval-hint">
        verify 对抗评审模糊时默认判 FAIL（gates.rs parse_verdict）。这里列出需人判的模糊 FAIL，给归因链①②③ +
        reviewer 原文 + 确认/误判/重跑，避免假绿或误杀。
      </div>
      {ambiguous.length === 0 ? (
        <p className="eval-empty">无模糊 FAIL 判决（所有 verify/honesty FAIL 都清晰，或尚无此类判决）。</p>
      ) : (
        ambiguous.map((v) => {
          const trace = parseVerdictTrace(v.report ?? '');
          const isExp = expanded[v.id];
          const judge = judged[v.id];
          return (
            <div key={v.id} className="eval-ambiguous-card">
              <div className="eval-fail-head">
                <b>{verdictTarget(v, caseName)}</b>
                <span className="eval-badge eval-badge-unclear">⚠ 模糊 FAIL</span>
                {judge === 'confirm' && <span className="eval-badge eval-badge-brake">人判 · 确认 FAIL</span>}
                {judge === 'dispute' && <span className="eval-badge eval-badge-clear">人判 · 误判（推翻）</span>}
              </div>

              <div className="eval-section-label">归因链（镜像后端 parse_verdict）</div>
              <div className="eval-trace-chain mono">
                {trace.steps.map((s, i) => (
                  <div key={i} className={`eval-chain-step ${s.hit ? 'hit' : 'miss'}`}>
                    <span className="eval-chain-mark">{s.hit ? '▸' : '·'}</span>
                    {s.label}
                  </div>
                ))}
                <div className={`eval-chain-outcome ${trace.passed ? 'pass' : 'fail'}`}>
                  判定：{trace.passed ? 'PASS' : 'FAIL'}
                </div>
              </div>

              <div className="eval-section-label">
                reviewer 原文（{isExp ? '全文' : '截断 300'}）
              </div>
              <div className="eval-locked mono">
                {isExp ? v.report ?? '(无 report)' : (v.report ?? '(无 report)').slice(0, 300)}
              </div>
              <button
                className="eval-link"
                onClick={() => setExpanded((e) => ({ ...e, [v.id]: !e[v.id] }))}
              >
                {isExp ? '收起' : '展开全文'}
              </button>

              <div className="eval-actions">
                <Button
                  variant="ghost"
                  onClick={() => setJudged((j) => ({ ...j, [v.id]: 'confirm' }))}
                >
                  ✓ 确认 FAIL
                </Button>
                <Button
                  variant="ghost"
                  onClick={() => setJudged((j) => ({ ...j, [v.id]: 'dispute' }))}
                >
                  ✗ 误判（推翻）
                </Button>
                <Button
                  variant="primary"
                  onClick={() => void doRerun(v)}
                  disabled={rerunning[v.id]}
                >
                  {rerunning[v.id] ? '重跑中…' : '▶ 重跑'}
                </Button>
                {rerunMsg[v.id] && <span className="eval-hint mono">{rerunMsg[v.id]}</span>}
              </div>
              <div className="eval-hint">
                人判反馈当前为本地态（未持久化）；重跑调 run_eval_replay 落新 eval verdict。
              </div>
            </div>
          );
        })
      )}
    </div>
  );
}

// ── F1 回归曲线 + 新旧版本对比 ──
function RegressionCurve({ trend, verdicts }: { trend: TrendPoint[]; verdicts: VerdictRow[] }) {
  // 版本对比卡片：eval_trend 后端无 version 维度，这里前端按 commit_sha 派生。
  // 取 eval gate 且带 commit_sha 的 verdicts，按 commit 分组算均分，最新两个
  // commit = 新/旧版本，delta 决定准入（admit/brake/hold）—— 反刷分 #3 配对回放
  // 的版本级视图（P5 是 case 级，F1 是 commit 级）。
  const versionCompare = useMemo(() => {
    const byCommit = new Map<string, { scores: number[]; latestAt: string }>();
    for (const v of verdicts) {
      if (v.gate !== 'eval' || !v.commit_sha) continue;
      const entry = byCommit.get(v.commit_sha) ?? { scores: [], latestAt: v.created_at };
      entry.scores.push(scoreOf(v));
      if (v.created_at > entry.latestAt) entry.latestAt = v.created_at;
      byCommit.set(v.commit_sha, entry);
    }
    if (byCommit.size < 2) return null;
    const commits = [...byCommit.entries()].sort((a, b) => b[1].latestAt.localeCompare(a[1].latestAt));
    const newC = commits[0];
    const oldC = commits[1];
    const newAvg = newC[1].scores.reduce((a, b) => a + b, 0) / newC[1].scores.length;
    const oldAvg = oldC[1].scores.reduce((a, b) => a + b, 0) / oldC[1].scores.length;
    const delta = newAvg - oldAvg;
    const admit: 'admit' | 'brake' | 'hold' = delta > 0.001 ? 'admit' : delta < -0.001 ? 'brake' : 'hold';
    return {
      newSha: newC[0],
      oldSha: oldC[0],
      newAvg,
      oldAvg,
      delta,
      admit,
      newN: newC[1].scores.length,
      oldN: oldC[1].scores.length,
    };
  }, [verdicts]);

  if (trend.length === 0 && !versionCompare) {
    return <p className="eval-empty">暂无评估数据。回放（P4）或对会话打分即生成首条轨迹评分。</p>;
  }

  const curve =
    trend.length < 2 ? (
      // 单点画不出趋势线 —— 诚实标注数据不足，不渲染一根伪趋势。
      <p className="eval-empty">
        仅 {trend.length} 天数据
        {trend.length === 1 && `（${trend[0].date} · 均分 ${trend[0].avg_score.toFixed(2)} · ${trend[0].count} 次）`}
        。回归曲线需 ≥2 天才能看趋势 —— 再回放几次（P4）补点。
      </p>
    ) : (
      <div className="eval-chart">
        <Line
          data={{
            labels: trend.map((p) => p.date),
            datasets: [
              {
                label: '平均得分',
                data: trend.map((p) => p.avg_score),
                fill: true,
                borderColor: 'var(--accent)',
                backgroundColor: 'rgba(75, 96, 124, 0.08)',
                tension: 0.3,
                pointRadius: 3,
              },
            ],
          }}
          options={{
            responsive: true,
            maintainAspectRatio: false,
            plugins: { legend: { display: false } },
            scales: { y: { min: 0, max: 1 }, x: {} },
          }}
        />
      </div>
    );

  return (
    <>
      <div className="eval-card">{curve}</div>
      {versionCompare && (
        <div className="eval-card">
          <div className="eval-section-label">新旧版本均分对比（按 commit_sha 派生 · 反刷分配对回放门）</div>
          <div className="eval-version-compare">
            <div className="eval-version-col old">
              <div className="eval-hint mono">旧 · {versionCompare.oldSha.slice(0, 7)}</div>
              <div className="eval-version-avg">{versionCompare.oldAvg.toFixed(3)}</div>
              <div className="eval-hint mono">{versionCompare.oldN} 次 eval</div>
            </div>
            <div className="eval-version-delta">
              <div className={`eval-version-delta-val ${versionCompare.admit}`}>
                {versionCompare.delta >= 0 ? '+' : ''}
                {versionCompare.delta.toFixed(3)}
              </div>
              {versionCompare.admit === 'admit' ? (
                <span className="eval-badge eval-badge-clear">净提升 · 可准入</span>
              ) : versionCompare.admit === 'brake' ? (
                <span className="eval-badge eval-badge-brake">回归 · 拦</span>
              ) : (
                <span className="eval-badge eval-badge-unclear">持平 · 待判</span>
              )}
            </div>
            <div className="eval-version-col new">
              <div className="eval-hint mono">新 · {versionCompare.newSha.slice(0, 7)}</div>
              <div className="eval-version-avg">{versionCompare.newAvg.toFixed(3)}</div>
              <div className="eval-hint mono">{versionCompare.newN} 次 eval</div>
            </div>
          </div>
          <div className="eval-hint">
            verdicts 已带 commit_sha（run_eval_replay 记录），前端按 commit 聚合均分；delta&gt;0 准入 / &lt;0 拦 / =0 待判。
            与 P5（case 级配对）互补：F1 是 commit 级整体回归门。
          </div>
        </div>
      )}
    </>
  );
}

// ── F2 飞轮闭环 ──
function Flywheel({
  caseCount,
  verdictCount,
  failCount,
}: {
  caseCount: number;
  verdictCount: number;
  failCount: number;
}) {
  const links: [string, number, string][] = [
    ['回放 P4/P5', verdictCount, 'eval-arc'],
    ['判决 V1', verdictCount, 'eval-arc'],
    ['失败资产 V2', failCount, 'eval-arc'],
    ['下轮 case P1', caseCount, 'eval-arc'],
  ];
  const broken = failCount === 0 && verdictCount > 0;
  return (
    <div className="eval-card">
      <div className="eval-flywheel">
        {links.map(([label, n], i) => (
          <div key={label} className="eval-flywheel-node">
            <div className="eval-flywheel-circle">
              {label}
              <div className="eval-flywheel-count">{n}</div>
            </div>
            {i < links.length - 1 && <span className="eval-flywheel-arrow">→</span>}
          </div>
        ))}
      </div>
      <div className="eval-actions" style={{ justifyContent: 'center' }}>
        {broken ? (
          <span className="eval-badge eval-badge-brake">断链 · 失败未沉淀（V2 空）</span>
        ) : caseCount === 0 ? (
          <span className="eval-badge eval-badge-unclear">飞轮停转 · 无 case（P1 空）</span>
        ) : (
          <span className="eval-badge eval-badge-clear">闭环运转中</span>
        )}
      </div>
      <div className="eval-hint">第 5 点 Harness Evolution：paired replay 准入防刷分自嗨。</div>
    </div>
  );
}

// ── A1 配对 Trace 树（左右双栏 span forest）──
function OtelTraces({ cases, verdicts }: { cases: EvalCaseRow[]; verdicts: VerdictRow[] }) {
  const readyCases = cases.filter((c) => !c.draft);
  const [caseId, setCaseId] = useState('');
  useEffect(() => {
    if (!caseId && readyCases.length > 0) setCaseId(readyCases[0].id);
  }, [readyCases, caseId]);

  // 取该 case 的 eval verdicts（带 session_id 才能重建轨迹），new-first：
  // [0]=新, [1]=旧（与 P5/PairedCompare 同序约定）。
  const evalVs = verdicts.filter((v) => v.gate === 'eval' && v.case_id === caseId && v.session_id);
  const [newV, oldV] = evalVs;

  const [newTraj, setNewTraj] = useState<FullTrajectory | null>(null);
  const [oldTraj, setOldTraj] = useState<FullTrajectory | null>(null);
  const [loading, setLoading] = useState(false);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    setNewTraj(null);
    setOldTraj(null);
    setErr(null);
    if (!newV?.session_id) return;
    let cancelled = false;
    setLoading(true);
    Promise.all([
      evalApi.previewTrajectory(newV.session_id),
      oldV?.session_id ? evalApi.previewTrajectory(oldV.session_id) : Promise.resolve(null),
    ])
      .then(([n, o]) => {
        if (cancelled) return;
        setNewTraj(n);
        setOldTraj(o);
      })
      .catch((e) => {
        if (!cancelled) setErr(String(e));
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [newV?.session_id, oldV?.session_id]);

  const spanCount = (t: FullTrajectory | null) =>
    t ? t.span_tree.roots.length + t.span_tree.roots.reduce((n, r) => n + (r.children?.length ?? 0), 0) : 0;
  const stepNames = (t: FullTrajectory | null) => t?.steps.map((s) => s.name) ?? [];

  return (
    <div className="eval-card">
      <div className="eval-section-label">选 Case（左右双栏渲染其新旧 eval 的 span 森林）</div>
      <select className="eval-input" value={caseId} onChange={(e) => setCaseId(e.target.value)}>
        {readyCases.length === 0 && <option value="">暂无已审核 case</option>}
        {readyCases.map((c) => (
          <option key={c.id} value={c.id}>
            {c.name}
          </option>
        ))}
      </select>

      {evalVs.length < 2 && (
        <p className="eval-empty">
          该 case 的 eval verdict 不足 2 条（{evalVs.length}）。至少回放两次（P4）才能左右配对 trace。
        </p>
      )}

      {loading && <p className="eval-empty">提取双栏 span 树…</p>}
      {err && <p className="eval-empty">提取失败：{err}</p>}

      {newTraj && oldTraj && (
        <div className="eval-paired-traces">
          <TraceCol
            title={`旧 · ${fmtTime(oldV.created_at)}`}
            traj={oldTraj}
            otherNames={stepNames(newTraj)}
          />
          <TraceCol
            title={`新 · ${fmtTime(newV.created_at)}`}
            traj={newTraj}
            otherNames={stepNames(oldTraj)}
            kind={netVerdict(oldV, newV)}
          />
        </div>
      )}
      {newTraj && oldTraj && (
        <div className="eval-compare-summary">
          {netVerdict(oldV, newV) === 'improve' ? (
            <span className="eval-badge eval-badge-clear">净提升 · 可准入</span>
          ) : netVerdict(oldV, newV) === 'regress' ? (
            <span className="eval-badge eval-badge-brake">回归 · 拦</span>
          ) : (
            <span className="eval-badge eval-badge-unclear">持平 · 待判</span>
          )}
          <span className="eval-hint mono">
            {' '}
            span 数：旧 {spanCount(oldTraj)} → 新 {spanCount(newTraj)}
          </span>
        </div>
      )}
      <div className="eval-hint">
        span 树从已记录的 HTTP trace 派生（LLM=父 ◐，tool=子 └）。左右双栏对齐新旧 eval 的 tid 串联——
        L4 paired 的可视化底座。OTLP 原生导出是后续工作。
      </div>
    </div>
  );
}

function TraceCol({
  title,
  traj,
  otherNames,
  kind,
}: {
  title: string;
  traj: FullTrajectory;
  otherNames: string[];
  kind?: string;
}) {
  const rootCount = traj.span_tree.roots.length;
  const toolSpanCount = traj.span_tree.roots.reduce((n, r) => n + (r.children?.length ?? 0), 0);
  // diff: 一个 tool step 名字只在本侧出现（另一侧没有）= 这侧独有的策略增量。
  const other = new Set(otherNames.map((s) => s.toLowerCase()));
  return (
    <div className={`eval-trace-col ${kind === 'regress' ? 'regress' : kind === 'improve' ? 'improve' : ''}`}>
      <div className="eval-col-head">
        <span>{title}</span>
        <span className="eval-hint mono">
          {rootCount} 父 · {toolSpanCount} 子
        </span>
      </div>
      <div className="eval-span-forest">
        {rootCount === 0 ? (
          <span className="eval-empty">无 span（trace 已清或纯文本轮）</span>
        ) : (
          traj.span_tree.roots.map((r, i) => (
            <SpanNode key={i} span={r} depth={0} diffOther={other} />
          ))
        )}
      </div>
    </div>
  );
}

function SpanNode({
  span,
  depth,
  diffOther,
}: {
  span: Span;
  depth: number;
  diffOther?: Set<string>;
}) {
  const isLlm = span.kind === 'llm';
  const failed = span.status === 'error';
  // tool span 名字在配对另一侧没有 → 这侧独有的策略增量，unique 高亮。
  const unique = !isLlm && diffOther != null && !diffOther.has(span.name.toLowerCase());
  return (
    <div
      className={`eval-span ${isLlm ? 'llm' : 'tool'} ${failed ? 'fail' : ''} ${unique ? 'unique' : ''}`}
      style={{ marginLeft: depth * 16 }}
    >
      <span className="eval-span-kind">{isLlm ? '◐ LLM' : '└ tool'}</span>
      <span className="eval-span-name mono">{span.name}</span>
      {span.latency_ms != null && <span className="eval-hint mono">{span.latency_ms}ms</span>}
      {failed && <span className="eval-badge eval-badge-brake">error</span>}
      {(span.children ?? []).map((c, i) => (
        <SpanNode key={i} span={c} depth={depth + 1} diffOther={diffOther} />
      ))}
    </div>
  );
}
