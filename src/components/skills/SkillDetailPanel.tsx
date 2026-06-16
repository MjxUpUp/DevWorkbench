import { useSkillStore } from '../../stores/skillStore';

function LevelBar({ label, value }: { label: string; value: number }) {
  const color =
    value >= 85 ? 'var(--status-success)' : value >= 65 ? 'var(--status-warning)' : 'var(--status-error)';

  return (
    <div className="skill-detail-level-row">
      <span className="skill-detail-level-label">{label}</span>
      <div className="skill-detail-level-track">
        <div className="skill-detail-level-fill" style={{ width: `${value}%`, background: color }} />
      </div>
      <span className="skill-detail-level-value">{value}%</span>
    </div>
  );
}

function StarRating({ rating }: { rating: number }) {
  return (
    <span className="skill-detail-stars">
      {Array.from({ length: 5 }, (_, i) => (
        <span key={i} className={i < Math.round(rating) ? 'star filled' : 'star'}>
          ★
        </span>
      ))}
      <span className="star-value">{rating.toFixed(1)}</span>
    </span>
  );
}

export function SkillDetailPanel() {
  const selectedSkill = useSkillStore((s) => s.selectedSkill);
  const selectSkill = useSkillStore((s) => s.selectSkill);
  const installSkill = useSkillStore((s) => s.installSkill);
  const uninstallSkill = useSkillStore((s) => s.uninstallSkill);

  if (!selectedSkill) return null;

  const categoryColorMap: Record<string, string> = {
    orchestration: 'var(--skill-orchestration)',
    quality: 'var(--skill-quality)',
    security: 'var(--skill-security)',
    efficiency: 'var(--skill-efficiency)',
  };
  const categoryColor = categoryColorMap[selectedSkill.category] ?? 'var(--accent)';

  return (
    <div className="skill-detail-panel">
      <div className="skill-detail-header">
        <button className="skill-detail-close" onClick={() => selectSkill(null)}>
          ✕
        </button>
        <span className="skill-detail-icon">{selectedSkill.icon}</span>
        <div className="skill-detail-title-area">
          <h3 className="skill-detail-name">{selectedSkill.name}</h3>
          <span className="skill-detail-version">v{selectedSkill.version}</span>
        </div>
      </div>

      <div className="skill-detail-meta">
        <span className="skill-detail-org">
          {selectedSkill.org}
          {selectedSkill.author && ` / ${selectedSkill.author}`}
        </span>
        <span className="skill-detail-category-tag" style={{ background: categoryColor }}>
          {selectedSkill.category}
        </span>
      </div>

      <div className="skill-detail-stats">
        <div className="skill-detail-stat">
          <StarRating rating={selectedSkill.rating} />
        </div>
        <div className="skill-detail-stat">
          <span className="skill-detail-stat-value">{selectedSkill.installs.toLocaleString()}</span>
          <span className="skill-detail-stat-label">installs</span>
        </div>
      </div>

      <p className="skill-detail-description">{selectedSkill.description}</p>

      {/* Compatible Agents */}
      {selectedSkill.compatibleAgents && selectedSkill.compatibleAgents.length > 0 && (
        <div className="skill-detail-section">
          <h4 className="skill-detail-section-title">Compatible Agents</h4>
          <div className="skill-detail-agents">
            {selectedSkill.compatibleAgents.map((agent) => (
              <span key={agent} className="skill-detail-agent-chip">
                {agent}
              </span>
            ))}
          </div>
        </div>
      )}

      {/* Quality Report */}
      {selectedSkill.qualityDetails && (
        <div className="skill-detail-section">
          <h4 className="skill-detail-section-title">
            Quality Assessment
            <span className="skill-detail-section-badge" style={{ background: 'var(--skill-quality)' }}>
              L{selectedSkill.qualityLevel}
            </span>
          </h4>
          <LevelBar label="L1 Structural" value={selectedSkill.qualityDetails.l1} />
          <LevelBar label="L2 Code Quality" value={selectedSkill.qualityDetails.l2} />
          <LevelBar label="L3 Semantic" value={selectedSkill.qualityDetails.l3} />
          <LevelBar label="L4 Behavioral" value={selectedSkill.qualityDetails.l4} />
        </div>
      )}

      {/* Security Report */}
      {selectedSkill.securityDetails && (
        <div className="skill-detail-section">
          <h4 className="skill-detail-section-title">
            Security Assessment
            <span className="skill-detail-section-badge" style={{ background: 'var(--skill-security)' }}>
              L{selectedSkill.securityLevel}
            </span>
          </h4>
          <LevelBar label="L1 Identity Trust" value={selectedSkill.securityDetails.l1} />
          <LevelBar label="L2 Static Analysis" value={selectedSkill.securityDetails.l2} />
          <LevelBar label="L3 Dynamic Analysis" value={selectedSkill.securityDetails.l3} />
          <LevelBar label="L4 Permission Model" value={selectedSkill.securityDetails.l4} />
        </div>
      )}

      {/* Actions */}
      <div className="skill-detail-actions">
        {selectedSkill.installed ? (
          <button
            className="skill-detail-btn uninstall"
            onClick={() => uninstallSkill(selectedSkill.id)}
          >
            Uninstall
          </button>
        ) : (
          <button
            className="skill-detail-btn install"
            onClick={() => installSkill(selectedSkill.id)}
          >
            Install
          </button>
        )}
        <button className="skill-detail-btn secondary">View Source</button>
      </div>
    </div>
  );
}
