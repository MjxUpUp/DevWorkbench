import { useEffect } from 'react';
import { useSkillStore, useFilteredSkills } from '../../stores/skillStore';
import { SkillSearchBar } from './SkillSearchBar';
import { SkillCard } from './SkillCard';
import { FeaturedBanner } from './FeaturedBanner';
import { SkillDetailPanel } from './SkillDetailPanel';

export function SkillMarketView() {
  const fetchSkills = useSkillStore((s) => s.fetchSkills);
  const filtered = useFilteredSkills();
  const selectedSkill = useSkillStore((s) => s.selectedSkill);
  const loading = useSkillStore((s) => s.loading);

  useEffect(() => { fetchSkills(); }, [fetchSkills]);

  return (
    <div className="skill-market-view">
      <div className="skill-market-main">
        <SkillSearchBar />
        <FeaturedBanner />

        {loading ? (
          <div className="skill-market-loading">Loading skills...</div>
        ) : filtered.length === 0 ? (
          <div className="skill-market-empty">No skills found</div>
        ) : (
          <div className="skill-market-grid">
            {filtered.map((skill) => (
              <SkillCard key={skill.id} skill={skill} />
            ))}
          </div>
        )}
      </div>

      {selectedSkill && (
        <div className="skill-market-drawer">
          <SkillDetailPanel />
        </div>
      )}
    </div>
  );
}
