import { useSkillStore, type SkillCategory, type SkillSort } from '../../stores/skillStore';

const CATEGORIES: { value: SkillCategory; label: string }[] = [
  { value: 'all', label: 'All' },
  { value: 'orchestration', label: 'Orchestration' },
  { value: 'quality', label: 'Quality' },
  { value: 'security', label: 'Security' },
  { value: 'efficiency', label: 'Efficiency' },
];

const SORT_OPTIONS: { value: SkillSort; label: string }[] = [
  { value: 'rating', label: 'Rating' },
  { value: 'installs', label: 'Installs' },
  { value: 'newest', label: 'Newest' },
];

export function SkillSearchBar() {
  const searchQuery = useSkillStore((s) => s.searchQuery);
  const selectedCategory = useSkillStore((s) => s.selectedCategory);
  const sortBy = useSkillStore((s) => s.sortBy);
  const search = useSkillStore((s) => s.search);
  const setCategory = useSkillStore((s) => s.setCategory);
  const setSortBy = useSkillStore((s) => s.setSortBy);

  return (
    <div className="skill-search-bar">
      <div className="skill-search-input-wrap">
        <span className="skill-search-icon">⌕</span>
        <input
          type="text"
          className="skill-search-input"
          placeholder="Search skills..."
          value={searchQuery}
          onChange={(e) => search(e.target.value)}
        />
      </div>

      <div className="skill-category-tabs">
        {CATEGORIES.map((cat) => (
          <button
            key={cat.value}
            className={`skill-category-tab ${selectedCategory === cat.value ? 'active' : ''}`}
            data-category={cat.value}
            onClick={() => setCategory(cat.value)}
          >
            {cat.label}
          </button>
        ))}
      </div>

      <div className="skill-sort-control">
        <span className="skill-sort-label">Sort:</span>
        <select
          className="skill-sort-select"
          value={sortBy}
          onChange={(e) => setSortBy(e.target.value as SkillSort)}
        >
          {SORT_OPTIONS.map((opt) => (
            <option key={opt.value} value={opt.value}>
              {opt.label}
            </option>
          ))}
        </select>
      </div>
    </div>
  );
}
