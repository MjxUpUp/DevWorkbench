import type { SkillItem } from '../../stores/skillStore';
import { useSkillStore } from '../../stores/skillStore';

export function FeaturedBanner() {
  const skills = useSkillStore((s) => s.skills);
  const selectSkill = useSkillStore((s) => s.selectSkill);

  // Pick the highest-rated skill as featured
  const featured: SkillItem | undefined = [...skills].sort((a, b) => b.rating - a.rating)[0];
  if (!featured) return null;

  const categoryColorMap: Record<string, string> = {
    orchestration: 'var(--skill-orchestration)',
    quality: 'var(--skill-quality)',
    security: 'var(--skill-security)',
    efficiency: 'var(--skill-efficiency)',
  };

  return (
    <div
      className="skill-featured-banner"
      style={{ borderLeftColor: categoryColorMap[featured.category] ?? 'var(--accent)' }}
    >
      <div className="skill-featured-badge">Featured</div>
      <div className="skill-featured-content">
        <span className="skill-featured-icon">{featured.icon}</span>
        <div className="skill-featured-info">
          <div className="skill-featured-title">
            {featured.name}
            <span className="skill-featured-version">v{featured.version}</span>
          </div>
          <p className="skill-featured-desc">{featured.description}</p>
          <div className="skill-featured-meta">
            <span className="skill-featured-rating">★ {featured.rating.toFixed(1)}</span>
            <span className="skill-featured-installs">{featured.installs.toLocaleString()} installs</span>
            <span className="skill-featured-org">by {featured.org}</span>
          </div>
        </div>
      </div>
      <button
        className="skill-featured-action"
        onClick={() => selectSkill(featured)}
      >
        View Details
      </button>
    </div>
  );
}
