import { useState, useEffect } from 'react';
import { useProvidersStore } from '../../stores/providersStore';
import { useToast } from '../Toast';
import type { ProviderConfig, ProvidersConfig } from '../../types';

export function ProvidersSection() {
  const { config, loading, loadProviders, saveProviders, testProvider } = useProvidersStore();
  const { info, error } = useToast();
  const [draft, setDraft] = useState<ProviderConfig[] | null>(null);
  const [testingId, setTestingId] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    loadProviders();
  }, [loadProviders]);

  // Sync local editable copy when the loaded config arrives/changes.
  useEffect(() => {
    if (config) setDraft(config.providers);
  }, [config]);

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

  const update = (id: string, patch: Partial<ProviderConfig>) => {
    setDraft(draft.map((p) => (p.id === id ? { ...p, ...patch } : p)));
  };

  // set_providers_config replaces the whole file, so every save persists the
  // entire draft (including edits to other cards).
  const handleSave = async () => {
    setSaving(true);
    try {
      const next: ProvidersConfig = {
        providers: draft,
        modelMapping: config?.modelMapping ?? {},
      };
      await saveProviders(next);
      info('供应商配置已保存');
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const handleTest = async (p: ProviderConfig) => {
    const model = p.models.find((m) => m.enabled)?.id ?? p.models[0]?.id ?? '';
    if (!model) {
      error('该供应商未配置模型');
      return;
    }
    setTestingId(p.id);
    try {
      const result = await testProvider(p.endpoint, p.apiKey, model);
      if (result.ok) info(`连接成功 (HTTP ${result.status})`);
      else error(`连接失败: ${result.message}`);
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    } finally {
      setTestingId(null);
    }
  };

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">模型供应商</h3>
      <p className="settings-section-desc">
        配置 AI 模型供应商和 API Key。透明内核 agent 会按模型自动匹配已启用且填了 Key 的供应商;未填 Key 时调用将以 401 失败。
      </p>
      <div className="provider-list">
        {draft.map((p) => (
          <div key={p.id} className="provider-card">
            <div className="provider-card-header">
              <span className="provider-name">{p.name}</span>
              <label className="provider-enabled-toggle">
                <input
                  type="checkbox"
                  checked={p.enabled}
                  onChange={(e) => update(p.id, { enabled: e.target.checked })}
                />
                <span>{p.enabled ? '已启用' : '已禁用'}</span>
              </label>
            </div>
            <div className="provider-fields">
              <div className="provider-field">
                <span className="provider-field-label">接口地址</span>
                <input
                  className="provider-field-input"
                  placeholder="https://..."
                  value={p.endpoint}
                  onChange={(e) => update(p.id, { endpoint: e.target.value })}
                />
              </div>
              <div className="provider-field">
                <span className="provider-field-label">API Key</span>
                <input
                  className="provider-field-input"
                  type="password"
                  placeholder="sk-..."
                  value={p.apiKey}
                  onChange={(e) => update(p.id, { apiKey: e.target.value })}
                />
              </div>
              <div className="provider-field">
                <span className="provider-field-label">可用模型</span>
                <span className="provider-models">
                  {p.models.map((m) => m.label).join(' / ') || '—'}
                </span>
              </div>
            </div>
            <div className="provider-actions">
              <button className="provider-btn primary" onClick={handleSave} disabled={saving}>
                {saving ? '保存中...' : '保存'}
              </button>
              <button
                className="provider-btn secondary"
                onClick={() => handleTest(p)}
                disabled={testingId === p.id}
              >
                {testingId === p.id ? '测试中...' : '测试连接'}
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
