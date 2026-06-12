export function ProvidersSection() {
  return (
    <div className="settings-section">
      <h3 className="settings-section-title">模型供应商</h3>
      <p className="settings-section-desc">配置 AI 模型供应商和 API Key，支持多供应商切换</p>
      <div className="provider-list">
        {[
          { name: 'Z.AI (GLM)', status: '预置', color: 'var(--accent)' },
          { name: 'BigModel', status: '预置', color: 'var(--accent)' },
          { name: 'Anthropic', status: '自定义', color: 'var(--text-muted)' },
          { name: 'DeepSeek', status: '自定义', color: 'var(--text-muted)' },
          { name: 'OpenRouter', status: '自定义', color: 'var(--text-muted)' },
        ].map(provider => (
          <div key={provider.name} className="provider-card">
            <div className="provider-card-header">
              <span className="provider-name">{provider.name}</span>
              <span className="provider-status disconnected">{provider.status}</span>
            </div>
            <div className="provider-fields">
              <div className="provider-field">
                <span className="provider-field-label">接口地址</span>
                <input className="provider-field-input" placeholder="https://api.example.com/v1" />
              </div>
              <div className="provider-field">
                <span className="provider-field-label">API Key</span>
                <input className="provider-field-input" type="password" placeholder="sk-..." />
              </div>
            </div>
            <div className="provider-actions">
              <button className="provider-btn primary">保存</button>
              <button className="provider-btn secondary">测试连接</button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
