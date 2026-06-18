import { useState, useEffect, useMemo } from 'react';
import { useProvidersStore } from '../../stores/providersStore';
import { useToast } from '../Toast';
import type { ProviderConfig, ProvidersConfig, ModelEntry } from '../../types';

/**
 * The logical model id the transparent kernel agent requests by default.
 * Mirrors executor.rs's `model.unwrap_or("glm-4.6")`. The "默认模型" dropdown
 * writes into modelMapping[DEFAULT_MODEL_ID] to redirect that request to a
 * different model when the user picks one.
 */
const DEFAULT_MODEL_ID = 'glm-4.6';

/** Per-provider probe result, kept in-card so users can compare and revisit
 * connectivity instead of chasing a transient toast. */
interface TestRecord {
  ok: boolean;
  status: number;
  message: string;
}
type TestResults = Record<string, TestRecord>; // key = providerId

function isValidUrl(s: string): boolean {
  if (!s.trim()) return false;
  try {
    const u = new URL(s);
    return u.protocol === 'http:' || u.protocol === 'https:';
  } catch {
    return false;
  }
}

/** Structural equality — when draft === config, nothing is unsaved. */
function configsEqual(a: ProvidersConfig, b: ProvidersConfig): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

function cloneConfig(c: ProvidersConfig): ProvidersConfig {
  return {
    providers: c.providers.map((p) => ({ ...p, models: p.models.map((m) => ({ ...m })) })),
    modelMapping: { ...c.modelMapping },
  };
}

export function ProvidersSection() {
  const { config, loading, loadProviders, saveProviders, testProvider } = useProvidersStore();
  const { error, success } = useToast();

  const [draft, setDraft] = useState<ProvidersConfig | null>(null);
  const [saving, setSaving] = useState(false);
  const [testResults, setTestResults] = useState<TestResults>({});
  const [activeTest, setActiveTest] = useState<string | null>(null);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [revealedKeys, setRevealedKeys] = useState<Set<string>>(new Set());

  useEffect(() => {
    loadProviders();
  }, [loadProviders]);

  // Sync a deep editable copy when the loaded config arrives.
  useEffect(() => {
    if (config) setDraft(cloneConfig(config));
  }, [config]);

  const dirty = useMemo(
    () => Boolean(draft && config && !configsEqual(draft, config)),
    [draft, config],
  );

  const providerDirty = (p: ProviderConfig): boolean => {
    if (!config) return true;
    const orig = config.providers.find((o) => o.id === p.id);
    return !orig || JSON.stringify(orig) !== JSON.stringify(p);
  };

  const errorFor = (providerId: string, field: string): string | undefined =>
    errors[`${providerId}:${field}`];

  // ---- draft mutation helpers ----
  const patchProvider = (id: string, patch: Partial<ProviderConfig>) => {
    setDraft((d) =>
      d ? { ...d, providers: d.providers.map((p) => (p.id === id ? { ...p, ...patch } : p)) } : d,
    );
  };
  const patchModel = (providerId: string, idx: number, patch: Partial<ModelEntry>) => {
    setDraft((d) =>
      d
        ? {
            ...d,
            providers: d.providers.map((p) =>
              p.id === providerId
                ? { ...p, models: p.models.map((m, i) => (i === idx ? { ...m, ...patch } : m)) }
                : p,
            ),
          }
        : d,
    );
  };
  const addModel = (providerId: string) => {
    setDraft((d) =>
      d
        ? {
            ...d,
            providers: d.providers.map((p) =>
              p.id === providerId
                ? { ...p, models: [...p.models, { id: `model-${crypto.randomUUID().slice(0, 8)}`, label: '新模型', enabled: true }] }
                : p,
            ),
          }
        : d,
    );
  };
  const removeModel = (providerId: string, idx: number) => {
    setDraft((d) =>
      d
        ? {
            ...d,
            providers: d.providers.map((p) =>
              p.id === providerId ? { ...p, models: p.models.filter((_, i) => i !== idx) } : p,
            ),
          }
        : d,
    );
  };
  const addProvider = () => {
    setDraft((d) =>
      d
        ? {
            ...d,
            providers: [
              ...d.providers,
              {
                id: crypto.randomUUID(),
                name: '新供应商',
                endpoint: '',
                apiKey: '',
                enabled: true,
                models: [],
              },
            ],
          }
        : d,
    );
  };
  const removeProvider = (id: string) => {
    setDraft((d) => {
      if (!d) return d;
      const removed = d.providers.find((p) => p.id === id);
      const providers = d.providers.filter((p) => p.id !== id);
      // Drop default-model mappings that pointed at a removed model.
      const removedModelIds = new Set(removed?.models.map((m) => m.id) ?? []);
      const modelMapping = Object.fromEntries(
        Object.entries(d.modelMapping).filter(([, v]) => !removedModelIds.has(v)),
      );
      return { providers, modelMapping };
    });
  };

  // ---- default model (drives modelMapping) ----
  const enabledModelOptions = useMemo(() => {
    if (!draft) return [];
    return draft.providers
      .filter((p) => p.enabled)
      .flatMap((p) =>
        p.models
          .filter((m) => m.enabled)
          .map((m) => ({ providerId: p.id, modelId: m.id, label: `${p.name} / ${m.label}` })),
      );
  }, [draft]);

  const currentDefault =
    draft?.modelMapping[DEFAULT_MODEL_ID] ??
    enabledModelOptions.find((m) => m.modelId === DEFAULT_MODEL_ID)?.modelId ??
    '';

  const setDefaultModel = (modelId: string) => {
    setDraft((d) => {
      if (!d) return d;
      const modelMapping = { ...d.modelMapping };
      if (!modelId || modelId === DEFAULT_MODEL_ID) delete modelMapping[DEFAULT_MODEL_ID];
      else modelMapping[DEFAULT_MODEL_ID] = modelId;
      return { ...d, modelMapping };
    });
  };

  // ---- save + validation ----
  const validate = (cfg: ProvidersConfig): Record<string, string> => {
    const errs: Record<string, string> = {};
    for (const p of cfg.providers) {
      if (!p.name.trim()) errs[`${p.id}:name`] = '名称不能为空';
      if (p.enabled) {
        if (!isValidUrl(p.endpoint)) errs[`${p.id}:endpoint`] = '需为合法 http(s) 地址';
        if (!p.apiKey.trim()) errs[`${p.id}:apiKey`] = '启用供应商必须填写 API Key';
      }
    }
    return errs;
  };

  const handleSave = async () => {
    if (!draft) return;
    const errs = validate(draft);
    setErrors(errs);
    if (Object.keys(errs).length > 0) {
      error('请修正标红的字段后再保存');
      return;
    }
    setSaving(true);
    try {
      await saveProviders(draft);
      setErrors({});
      success('供应商配置已保存');
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  // ---- connectivity probe ----
  const handleTest = async (p: ProviderConfig, modelId: string) => {
    if (!modelId) {
      error('该供应商未配置可测模型');
      return;
    }
    setActiveTest(p.id);
    try {
      const r = await testProvider(p.endpoint, p.apiKey, modelId);
      setTestResults((prev) => ({ ...prev, [p.id]: { ok: r.ok, status: r.status, message: r.message } }));
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e);
      setTestResults((prev) => ({ ...prev, [p.id]: { ok: false, status: 0, message: msg } }));
    } finally {
      setActiveTest(null);
    }
  };

  const toggleReveal = (id: string) => {
    setRevealedKeys((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  // ---- render guards ----
  if (loading && !draft) {
    return (
      <div className="settings-section">
        <h3 className="settings-section-title">模型供应商</h3>
        <p className="settings-section-desc">加载中...</p>
      </div>
    );
  }
  if (!draft) {
    return (
      <div className="settings-section">
        <h3 className="settings-section-title">模型供应商</h3>
        <p className="settings-section-desc">无法加载供应商配置,请检查应用数据目录权限</p>
      </div>
    );
  }

  return (
    <div className="settings-section providers-section">
      <h3 className="settings-section-title">模型供应商</h3>
      <p className="settings-section-desc">
        配置 AI 模型供应商与 API Key。透明内核 agent 按模型自动匹配已启用且填了 Key 的供应商;未填 Key 时调用将以 401 失败。可新增自定义供应商、增删模型、选择默认模型。
      </p>

      {/* Default model — redirects the kernel's default model id via modelMapping */}
      <div className="provider-default-row">
        <label className="provider-field-label">默认模型</label>
        <select
          className="provider-default-select"
          aria-label="默认模型"
          value={currentDefault}
          onChange={(e) => setDefaultModel(e.target.value)}
          disabled={enabledModelOptions.length === 0}
        >
          {enabledModelOptions.length === 0 && <option value="">暂无已启用的模型</option>}
          {enabledModelOptions.map((m) => (
            <option key={`${m.providerId}:${m.modelId}`} value={m.modelId}>
              {m.label}
            </option>
          ))}
        </select>
        <span className="provider-default-hint">
          {currentDefault && currentDefault !== DEFAULT_MODEL_ID
            ? `将 "${DEFAULT_MODEL_ID}" 映射到 ${currentDefault}`
            : '内核默认请求模型'}
        </span>
      </div>

      {/* Section-level save bar: one save persists the whole file, so it lives
          here — not on each card (which would mislead users into thinking a
          per-card save only touches that card). */}
      <div className="provider-save-bar">
        {dirty && <span className="provider-dirty-badge">有未保存的更改</span>}
        <button
          className="provider-btn primary"
          onClick={handleSave}
          disabled={saving || !dirty}
        >
          {saving ? '保存中...' : dirty ? '保存全部更改' : '已保存'}
        </button>
      </div>

      <div className="provider-list">
        {draft.providers.map((p) => (
          <ProviderCard
            key={p.id}
            provider={p}
            dirty={providerDirty(p)}
            errorFor={(field) => errorFor(p.id, field)}
            testResult={testResults[p.id]}
            testing={activeTest === p.id}
            revealed={revealedKeys.has(p.id)}
            onPatch={(patch) => patchProvider(p.id, patch)}
            onPatchModel={(idx, patch) => patchModel(p.id, idx, patch)}
            onAddModel={() => addModel(p.id)}
            onRemoveModel={(idx) => removeModel(p.id, idx)}
            onRemove={() => removeProvider(p.id)}
            onTest={(modelId) => handleTest(p, modelId)}
            onToggleReveal={() => toggleReveal(p.id)}
          />
        ))}
      </div>

      <button className="provider-add-btn" onClick={addProvider}>
        + 添加供应商
      </button>
    </div>
  );
}

/** Switch-styled checkbox, reused for provider + model enabled flags. */
function Toggle({
  checked,
  onChange,
  ariaLabel,
}: {
  checked: boolean;
  onChange: (v: boolean) => void;
  ariaLabel: string;
}) {
  return (
    <label className="provider-toggle" aria-label={ariaLabel}>
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
      <span className="provider-toggle-slider" />
    </label>
  );
}

interface ProviderCardProps {
  provider: ProviderConfig;
  dirty: boolean;
  errorFor: (field: string) => string | undefined;
  testResult: TestRecord | undefined;
  testing: boolean;
  revealed: boolean;
  onPatch: (patch: Partial<ProviderConfig>) => void;
  onPatchModel: (idx: number, patch: Partial<ModelEntry>) => void;
  onAddModel: () => void;
  onRemoveModel: (idx: number) => void;
  onRemove: () => void;
  onTest: (modelId: string) => void;
  onToggleReveal: () => void;
}

function ProviderCard({
  provider: p,
  dirty,
  errorFor,
  testResult,
  testing,
  revealed,
  onPatch,
  onPatchModel,
  onAddModel,
  onRemoveModel,
  onRemove,
  onTest,
  onToggleReveal,
}: ProviderCardProps) {
  // Local: which model the connectivity probe targets (default = first enabled).
  const firstEnabledModel = p.models.find((m) => m.enabled)?.id ?? p.models[0]?.id ?? '';
  const [testModel, setTestModel] = useState(firstEnabledModel);
  // Sync testModel when the model list changes. useState's initial value runs
  // only ONCE at mount, so a provider created in-session mounts with models=[]
  // → testModel='' and stays '' even after the user adds a model (the card's
  // key={p.id} is stable, so adding a model re-renders without remounting).
  // That left the 测试连接 button permanently disabled (disabled={... || !testModel})
  // for freshly-added providers — the "新增的供应商没办法测试链接" symptom. Re-sync
  // when the selection becomes empty/stale, falling back to the first enabled.
  useEffect(() => {
    setTestModel((prev) => {
      if (prev && p.models.some((m) => m.id === prev)) return prev;
      return firstEnabledModel;
    });
  }, [firstEnabledModel, p.models]);

  const endpointErr = errorFor('endpoint');
  const apiKeyErr = errorFor('apiKey');
  const nameErr = errorFor('name');

  return (
    <div className={`provider-card${dirty ? ' dirty' : ''}${p.enabled ? '' : ' disabled'}`}>
      <div className="provider-card-header">
        <input
          className={`provider-name-input${nameErr ? ' invalid' : ''}`}
          value={p.name}
          aria-label="供应商名称"
          onChange={(e) => onPatch({ name: e.target.value })}
        />
        <div className="provider-card-header-actions">
          <Toggle
            checked={p.enabled}
            onChange={(v) => onPatch({ enabled: v })}
            ariaLabel={p.enabled ? '禁用该供应商' : '启用该供应商'}
          />
          <span className="provider-enabled-text">{p.enabled ? '已启用' : '已禁用'}</span>
          <button
            className="provider-icon-btn provider-delete-btn"
            aria-label={`删除供应商 ${p.name}`}
            onClick={onRemove}
          >
            删除
          </button>
        </div>
      </div>

      <div className="provider-fields">
        <div className="provider-field">
          <span className="provider-field-label">接口地址</span>
          <div className="provider-field-control">
            <input
              className={`provider-field-input${endpointErr ? ' invalid' : ''}`}
              placeholder="https://..."
              value={p.endpoint}
              aria-label="接口地址"
              onChange={(e) => onPatch({ endpoint: e.target.value })}
              disabled={!p.enabled}
            />
            {endpointErr && <span className="provider-field-error">{endpointErr}</span>}
          </div>
        </div>

        <div className="provider-field">
          <span className="provider-field-label">API Key</span>
          <div className="provider-field-control">
            <div className="provider-key-wrap">
              <input
                className={`provider-field-input${apiKeyErr ? ' invalid' : ''}`}
                type={revealed ? 'text' : 'password'}
                placeholder="sk-..."
                value={p.apiKey}
                aria-label="API Key"
                onChange={(e) => onPatch({ apiKey: e.target.value })}
                disabled={!p.enabled}
              />
              <button
                type="button"
                className="provider-key-reveal"
                aria-label={revealed ? '隐藏密钥' : '显示密钥'}
                onClick={onToggleReveal}
              >
                {revealed ? '隐藏' : '显示'}
              </button>
              <span className={`provider-key-badge${p.apiKey ? ' configured' : ''}`}>
                {p.apiKey ? '已配置' : '未配置'}
              </span>
            </div>
            {apiKeyErr && <span className="provider-field-error">{apiKeyErr}</span>}
          </div>
        </div>

        <div className="provider-field provider-models-field">
          <span className="provider-field-label">可用模型</span>
          <div className="provider-field-control">
            {p.models.length === 0 && (
              <span className="provider-models-empty">该供应商暂无模型,点击下方添加</span>
            )}
            <div className="provider-models-list">
              {p.models.map((m, idx) => (
                <div key={`${p.id}-model-${idx}`} className="provider-model-row">
                  <Toggle
                    checked={m.enabled}
                    onChange={(v) => onPatchModel(idx, { enabled: v })}
                    ariaLabel={m.enabled ? '禁用该模型' : '启用该模型'}
                  />
                  <input
                    className="provider-model-input"
                    value={m.label}
                    aria-label="模型显示名"
                    placeholder="显示名"
                    onChange={(e) => onPatchModel(idx, { label: e.target.value })}
                  />
                  <input
                    className="provider-model-input provider-model-id"
                    value={m.id}
                    aria-label="模型 ID"
                    placeholder="模型 id (如 glm-4.6)"
                    onChange={(e) => onPatchModel(idx, { id: e.target.value })}
                  />
                  <input
                    className="provider-model-input provider-model-window"
                    type="number"
                    min={0}
                    step={1000}
                    value={m.contextWindow ?? ''}
                    aria-label="上下文窗口"
                    placeholder="窗口 tokens (如 128000)"
                    title="模型上下文窗口（tokens）。留空则后端用保守默认 32k；auto-compact 在 75% 处触发"
                    onChange={(e) => {
                      const v = e.target.value;
                      // Empty → undefined (backend Option None → 32k default).
                      // Otherwise coerce to a non-negative int; NaN/garbage → 0
                      // which the backend guards (compact_threshold(0)=24k fallback).
                      onPatchModel(idx, {
                        contextWindow: v === '' ? undefined : Math.max(0, Math.floor(Number(v) || 0)),
                      });
                    }}
                  />
                  <button
                    className="provider-icon-btn provider-model-remove"
                    aria-label={`删除模型 ${m.label}`}
                    onClick={() => onRemoveModel(idx)}
                  >
                    ×
                  </button>
                </div>
              ))}
            </div>
            <button className="provider-add-model-btn" onClick={onAddModel}>
              + 添加模型
            </button>
          </div>
        </div>
      </div>

      {/* Connectivity probe: model selector + test button + in-card result */}
      <div className="provider-test-row">
        <select
          className="provider-test-select"
          aria-label={`测试模型 - ${p.name}`}
          value={testModel}
          onChange={(e) => setTestModel(e.target.value)}
          disabled={p.models.length === 0}
        >
          {p.models.length === 0 && <option value="">无模型</option>}
          {p.models.map((m, idx) => (
            <option key={`${p.id}-test-${idx}`} value={m.id}>
              {m.label}
            </option>
          ))}
        </select>
        <button
          className="provider-btn secondary"
          onClick={() => onTest(testModel)}
          disabled={testing || !testModel}
        >
          {testing ? `测试中 (${testModel})` : '测试连接'}
        </button>
        {testing && (
          <span className="provider-test-context">
            正在测试 {testModel} @ {p.endpoint || '(未填)'}
          </span>
        )}
        {testResult && !testing && (
          <span className={`provider-test-result ${testResult.ok ? 'ok' : 'fail'}`}>
            {testResult.ok ? `✓ 连通 (HTTP ${testResult.status})` : `✗ ${testResult.message}`}
          </span>
        )}
      </div>
    </div>
  );
}
