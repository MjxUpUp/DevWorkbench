import type { SkillItem } from '../../stores/skillStore';
import { useSkillStore } from '../../stores/skillStore';

function ScoreBar({ value, color }: { value: number; color: string }) {
  return (
    <div className="skill-score-bar-track">
      <div
        className="skill-score-bar-fill"
        style={{ width: `${value}%`, background: color }}
      />
    </div>
  );
}

function QualityBadge({ level }: { level: number }) {
  const colors: Record<number, string> = {
    1: '#9ca3af',
    2: '#3b82f6',
    3: '#22c55e',
    4: '#8b5cf6',
  };
  return (
    <span className="skill-level-badge" style={{ background: colors[level] ?? '#9ca3af' }}>
      L{level}
    </span>
  );
}

function formatInstalls(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

export function SkillCard({ skill }: { skill: SkillItem }) {
  const selectSkill = useSkillStore((s) => s.selectSkill);
  const selectedSkill = useSkillStore((s) => s.selectedSkill);
  const installSkill = useSkillStore((s) => s.installSkill);

  const isSelected = selectedSkill?.id === skill.id;

  const categoryColorMap: Record<string, string> = {
    orchestration: 'var(--skill-orchestration)',
    quality: 'var(--skill-quality)',
    security: 'var(--skill-security)',
    efficiency: 'var(--skill-efficiency)',
  };
  const categoryColor = categoryColorMap[skill.category] ?? 'var(--accent)';

  return (
    <div
      className={`skill-card ${isSelected ? 'selected' : ''}`}
      onClick={() => selectSkill(isSelected ? null : skill)}
    >
      <div className="skill-card-header">
        <span className="skill-card-icon">{skill.icon}</span>
        <div className="skill-card-title-area">
          <span className="skill-card-name">{skill.name}</span>
          <span className="skill-card-org">{skill.org}</span>
        </div>
        {skill.installed && <span className="skill-card-installed-badge">Installed</span>}
      </div>

      <p className="skill-card-desc">{skill.description}</p>

      <div className="skill-card-scores">
        <div className="skill-score-row">
          <div className="skill-score-label">
            <QualityBadge level={skill.qualityLevel} />
            <span>Quality</span>
          </div>
          <ScoreBar value={skill.qualityScore} color="var(--skill-quality)" />
          <span className="skill-score-value">{skill.qualityScore}</span>
        </div>
        <div className="skill-score-row">
          <div className="skill-score-label">
            <QualityBadge level={skill.securityLevel} />
            <span>Security</span>
          </div>
          <ScoreBar value={skill.securityScore} color="var(--skill-security)" />
          <span className="skill-score-value">{skill.securityScore}</span>
        </div>
      </div>

      <div className="skill-card-footer">
        <span className="skill-card-installs">{formatInstalls(skill.installs)} installs</span>
        <span className="skill-card-category-dot" style={{ background: categoryColor }} />
        <span className="skill-card-category">{skill.category}</span>
        {!skill.installed && (
          <button
            className="skill-card-install-btn"
            onClick={(e) => {
              e.stopPropagation();
              installSkill(skill.id);
            }}
          >
            Install
          </button>
        )}
      </div>
    </div>
  );
}
