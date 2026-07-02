import { useCallback, useEffect, useMemo, useState } from 'react';
import { Line } from 'react-chartjs-2';
import {
  evalApi,
  type TrendPoint,
  type VerdictRow,
  type EvalCaseRow,
  type ReplayVerdict,
  type ToolStep,
  type Matcher,
  type CreateCaseInput,
} from '../../utils/evalApi';
import { useAgentStore } from '../../stores/agentStore';
import { useNavigationStore } from '../../stores/navigationStore';
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
type FeatureId = 'P1' | 'P2' | 'P3' | 'P4' | 'P5' | 'P6' | 'V1' | 'V2' | 'V3' | 'F1' | 'F2' | 'A1';

const GROUPS: { title: string; items: FeatureId[] }[] = [
  { title: 'P · Case 池 & 回放', items: ['P1', 'P2', 'P3', 'P4', 'P5', 'P6'] },
  { title: 'V · 判决 & 失败资产', items: ['V1', 'V2', 'V3'] },
  { title: 'F · 飞轮', items: ['F1', 'F2'] },
  { title: 'A · OTel', items: ['A1'] },
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
          <CasePool cases={data.cases} onSelect={pickCase} onNewCase={() => setSelected('P3')} />
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
        {selected === 'F1' && <RegressionCurve trend={data.trend} />}
        {selected === 'F2' && (
          <Flywheel caseCount={data.cases.length} verdictCount={data.verdicts.length} failCount={failCount} />
        )}
        {selected === 'A1' && <OtelTraces />}
      </section>
    </div>
  );
}

// ── P1 Case 池 ──
function CasePool({
  cases,
  onSelect,
  onNewCase,
}: {
  cases: EvalCaseRow[];
  onSelect: (id: string) => void;
  onNewCase: () => void;
}) {
  const [q, setQ] = useState('');
  const [filter, setFilter] = useState<'all' | 'ready' | 'probe' | 'backlog'>('all');

  const status = (c: EvalCaseRow): 'ready' | 'probe' | 'backlog' =>
    c.category === 'backlog' ? 'backlog' : c.draft ? 'probe' : 'ready';

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
            return (
              <button key={c.id} className="eval-case-row" onClick={() => onSelect(c.id)}>
                <span className={`eval-badge eval-badge-${s === 'ready' ? 'clear' : s === 'probe' ? 'unclear' : 'muted'}`}>
                  {s}
                </span>
                <span className="eval-case-title">{c.name}</span>
                <span className="eval-case-meta mono">
                  {c.source_session_id ? `#${c.source_session_id.slice(0, 8)}` : '—'}
                  {c.commit_sha ? ` · ${c.commit_sha.slice(0, 7)}` : ''}
                </span>
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
      <div className="eval-hint mono">
        当前解析：{parseSteps(draft.expected_steps_json).map((s) => s.name).join(' → ') || '（空）'}
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
  const [steps, setSteps] = useState<ToolStep[] | null>(null);
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
    setSteps(null);
    setMsg(null);
    try {
      const s = await evalApi.previewTrajectory(sessionId);
      setSteps(s);
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
    if (!sessionId || !steps) return;
    setMsg(null);
    try {
      const sess = finished.find((x) => x.id === sessionId);
      const input: CreateCaseInput = {
        name: name || '未命名 case',
        category: 'agent',
        inputPrompt: sess?.prompt ?? '(无提示词)',
        expectedStepsJson: JSON.stringify(steps),
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
      {steps && (
        <div className="eval-steps-box">
          <div className="eval-hint mono">提取到 {steps.length} 步轨迹</div>
          <div className="eval-step-list">
            {steps.length === 0 ? (
              <span className="eval-empty">该会话无工具调用（纯文本轮）</span>
            ) : (
              steps.map((s, i) => (
                <div key={i} className="eval-step">
                  <span className="eval-step-idx">{i + 1}</span>
                  <span className={`eval-tool-tag ${s.status === 'error' ? 'fail' : ''}`}>{s.name}</span>
                </div>
              ))
            )}
          </div>
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
        <Button variant="primary" onClick={() => save(false)} disabled={!steps}>
          入库为 ready
        </Button>
        <Button variant="ghost" onClick={() => save(true)} disabled={!steps}>
          存为 probe
        </Button>
        {msg && <span className="eval-hint">{msg}</span>}
      </div>
    </div>
  );
}

// ── P4 评测任务配置（4 类评测对象）──
type EvalObject = 'agent' | 'platform-mechanism' | 'platform-e2e' | 'platform-enablement';

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
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState<ReplayVerdict | null>(null);
  const [err, setErr] = useState<string | null>(null);

  useEffect(() => {
    if (!caseId && readyCases.length > 0) setCaseId(readyCases[0].id);
  }, [readyCases, caseId]);

  useEffect(() => {
    setWorkingDir(workingFallback);
  }, [workingFallback]);

  const platformReady = obj === 'agent';

  async function run() {
    if (!caseId || !workingDir) return;
    setRunning(true);
    setErr(null);
    setResult(null);
    try {
      const v = await evalApi.runReplay(caseId, workingDir, matcher);
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
            ['platform-mechanism', '平台-机制', 'DAG/gate/compaction 行为 · 后端'],
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
      {!platformReady && (
        <div className="eval-gap-note">
          ⚠「{obj}」需平台评测驱动（L3 扩展 / 复用 playwright harness），当前未接入。下方回放仅 Agent 类可用。
        </div>
      )}

      <div className="eval-section-label">② Case（已审核）</div>
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

      <div className="eval-section-label">③ 工作区 / 匹配器</div>
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
        <Button variant="primary" onClick={run} disabled={running || !caseId || !workingDir || !platformReady}>
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
  const [caseId, setCaseId] = useState('');
  const readyCases = cases.filter((c) => !c.draft);
  useEffect(() => {
    if (!caseId && readyCases.length > 0) setCaseId(readyCases[0].id);
  }, [readyCases, caseId]);

  const evalVerdicts = verdicts.filter((v) => v.gate === 'eval' && v.case_id === caseId);
  const [oldV, newV] = evalVerdicts; // list_verdicts 是 new-first：[0]=新, [1]=旧

  return (
    <div className="eval-card">
      <div className="eval-section-label">选 Case（按 case_id 取其 eval verdicts，新旧各一）</div>
      <select className="eval-input" value={caseId} onChange={(e) => setCaseId(e.target.value)}>
        {readyCases.map((c) => (
          <option key={c.id} value={c.id}>
            {c.name}
          </option>
        ))}
      </select>

      {evalVerdicts.length < 2 ? (
        <p className="eval-empty">
          该 case 的 eval verdict 不足 2 条（{evalVerdicts.length}）。至少回放两次（P4）才能配对对比。
        </p>
      ) : (
        <div className="eval-compare">
          <CompareCol title={`旧 · ${fmtTime(oldV.created_at)}`} v={oldV} kind="old" />
          <CompareCol title={`新 · ${fmtTime(newV.created_at)}`} v={newV} kind={newScore(oldV, newV)} />
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

function scoreOf(v: VerdictRow): number {
  try {
    const r = JSON.parse(v.report ?? '{}') as { score?: number };
    return typeof r.score === 'number' ? r.score : v.verdict === 'PASS' ? 1 : 0;
  } catch {
    return 0;
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

function CompareCol({ title, v, kind }: { title: string; v: VerdictRow; kind: string }) {
  const steps: string[] = (() => {
    try {
      const r = JSON.parse(v.report ?? '{}') as { actual_steps?: string[] };
      return r.actual_steps ?? [];
    } catch {
      return [];
    }
  })();
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
          steps.map((s, i) => (
            <div key={i} className="eval-step">
              <span className="eval-step-idx">{i + 1}</span>
              <span className="eval-tool-tag">{s}</span>
            </div>
          ))
        )}
      </div>
      <AttributionBadge value={v.attribution} />
    </div>
  );
}

// ── P6 rubric ──
function RubricCard({
  verdicts,
  caseName,
}: {
  verdicts: VerdictRow[];
  caseName: (id: string | null) => string;
}) {
  const evalVs = verdicts.filter((v) => v.gate === 'eval');
  const latest = evalVs[0];

  return (
    <div className="eval-card">
      {latest ? (
        <>
          <div className="eval-hint mono">
            最近一次 eval verdict · {verdictTarget(latest, caseName)} · {fmtTime(latest.created_at)}
          </div>
          <div className="eval-rubric">
            <RubricRow name="工具选择准确率" pct={scoreOf(latest) * 100} val={scoreOf(latest).toFixed(2)} />
            <RubricRow
              name="反例命中（硬门）"
              pct={isNegViolated(latest) ? 0 : 100}
              val={isNegViolated(latest) ? '=0 命中' : '无'}
              hard
            />
            <RubricRow name="attribution（因果归因）" pct={latest.attribution === 'CLEAR' ? 100 : 30} val={latest.attribution ?? '—'} />
          </div>
          <div className="eval-gap-note">
            ⚠ AgentX 8 维（attribute hallucination / correctness-loop / harness-pattern / DSL / dryrun
            等）需 scoring.rs 扩展，当前后端仅 3 态 grade + 反例硬门。已展示可计算维度。
          </div>
        </>
      ) : (
        <p className="eval-empty">尚无 eval verdict。先在 P4 跑一次回放。</p>
      )}
    </div>
  );
}

function isNegViolated(v: VerdictRow): boolean {
  try {
    const r = JSON.parse(v.report ?? '{}') as { negative_violated?: boolean };
    return r.negative_violated === true;
  } catch {
    return false;
  }
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

// ── V2 失败资产（从 FAIL/TRIPPED/BRAKE 派生）──
function FailureAssets({
  verdicts,
  caseName,
}: {
  verdicts: VerdictRow[];
  caseName: (id: string | null) => string;
}) {
  const fails = verdicts.filter(
    (v) => v.verdict === 'FAIL' || v.verdict === 'TRIPPED' || v.attribution === 'BRAKE',
  );

  // 按 gate + 摘要 reason 分组（reason 来自 eval report；其它 gate 用 verdict 当键）。
  const groups = useMemo(() => {
    const m = new Map<string, VerdictRow[]>();
    for (const v of fails) {
      let reason = v.verdict;
      try {
        const r = JSON.parse(v.report ?? '{}') as { reason?: string };
        if (r.reason) reason = r.reason;
      } catch {
        /* keep verdict */
      }
      const key = `${v.gate} · ${reason}`.slice(0, 80);
      const arr = m.get(key) ?? [];
      arr.push(v);
      m.set(key, arr);
    }
    return [...m.entries()].sort((a, b) => b[1].length - a[1].length);
  }, [fails]);

  return (
    <div className="eval-card">
      <div className="eval-hint">
        失败资产从 FAIL / TRIPPED / BRAKE 判决派生（按 gate + reason 聚合）。下次 case 生成前检索规避。
      </div>
      {groups.length === 0 ? (
        <p className="eval-empty">尚无失败沉淀（无 FAIL/TRIPPED/BRAKE 判决）。</p>
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
                  .slice(0, 3)
                  .map((v) => verdictTarget(v, caseName))
                  .join(' · ')}
              </div>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

// ── V3 模糊 FAIL ──
function AmbiguousFail({
  verdicts,
  caseName,
}: {
  verdicts: VerdictRow[];
  caseName: (id: string | null) => string;
}) {
  // 模糊 FAIL = verify/honesty 的 FAIL，且 report 含「模糊/未含标准/默认/fallback」线索。
  const ambiguous = verdicts.filter((v) => {
    if (v.verdict !== 'FAIL' || !['verify', 'honesty'].includes(v.gate)) return false;
    const blob = `${v.report ?? ''}`.toLowerCase();
    return ['模糊', '未含标准', '默认', 'fallback', 'ambiguous'].some((k) => blob.includes(k));
  });

  return (
    <div className="eval-card">
      <div className="eval-hint">
        verify 对抗评审模糊时默认判 FAIL（gates.rs parse_verdict）。这里列出需人判的模糊 FAIL，给归因链 + 原文，避免假绿或误杀。
      </div>
      {ambiguous.length === 0 ? (
        <p className="eval-empty">无模糊 FAIL 判决（所有 verify/honesty FAIL 都清晰，或尚无此类判决）。</p>
      ) : (
        ambiguous.map((v) => (
          <div key={v.id} className="eval-ambiguous-card">
            <div className="eval-fail-head">
              <b>{verdictTarget(v, caseName)}</b>
              <span className="eval-badge eval-badge-unclear">⚠ 模糊 FAIL</span>
            </div>
            <div className="eval-section-label">归因链 / report（截断）</div>
            <div className="eval-locked mono">{(v.report ?? '(无 report)').slice(0, 300)}</div>
          </div>
        ))
      )}
    </div>
  );
}

// ── F1 回归曲线 ──
function RegressionCurve({ trend }: { trend: TrendPoint[] }) {
  if (trend.length === 0) {
    return <p className="eval-empty">暂无评估数据。回放（P4）或对会话打分即生成首条轨迹评分。</p>;
  }
  const data = {
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
  };
  const options = {
    responsive: true,
    maintainAspectRatio: false,
    plugins: { legend: { display: false } },
    scales: {
      y: { min: 0, max: 1 },
      x: {},
    },
  };
  return (
    <div className="eval-card">
      <div className="eval-chart">
        <Line data={data} options={options} />
      </div>
    </div>
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

// ── A1 OTel（诚实空壳）──
function OtelTraces() {
  return (
    <div className="eval-card">
      <div className="eval-gap-note">
        A1 OTel 数据层未接入（零起步）。现状：trace/ 是自定义 HTTP 日志，无 span 树 / OTLP。
        计划：LLM 调用=父 span，tool 调用=子 span，tid 串联左右配对会话，给 L4 paired 提供对齐底座。
      </div>
      <p className="eval-empty">配对 Trace 树视图依赖 A1 接入后渲染。</p>
    </div>
  );
}
