export interface SkillEntry {
  name: string;
  source: string;
  desc: string;
}

const SKILLS: SkillEntry[] = [
  { name: '/forge-pipeline', source: 'Forge', desc: '运行项目级质量管道' },
  { name: '/forge-quality', source: 'Forge', desc: '查看完整质量协议' },
  { name: '/plan', source: '内置', desc: '计划模式 — 先输出计划再执行' },
  { name: '/review', source: '内置', desc: '代码审查' },
  { name: '/test', source: '内置', desc: '运行测试' },
  { name: '/fix', source: '内置', desc: '修复问题' },
];

interface SkillListProps {
  onSelect?: (skill: SkillEntry) => void;
}

export function SkillList({ onSelect }: SkillListProps) {
  return (
    <div className="skill-list">
      {SKILLS.map(skill => (
        <div
          key={skill.name}
          className="skill-item"
          onClick={() => onSelect?.(skill)}
          style={{ cursor: onSelect ? 'pointer' : 'default' }}
        >
          <span className="skill-name">{skill.name}</span>
          <span className="skill-source">{skill.source}</span>
          <span className="skill-desc">{skill.desc}</span>
        </div>
      ))}
    </div>
  );
}

export { SKILLS };
