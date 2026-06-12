import type { AgentType } from '../types';

export interface TaskTemplate {
  id: string;
  icon: string;
  title: string;
  desc: string;
  prompt: string;
  recommendedAgent?: AgentType;
  recommendedMode?: string;
}

const TEMPLATES: TaskTemplate[] = [
  {
    id: 'new-feature',
    icon: '✨',
    title: '新建功能',
    desc: '实现一个新功能特性',
    prompt: '请帮我实现以下功能：',
    recommendedMode: 'plan',
  },
  {
    id: 'fix-bug',
    icon: '🐛',
    title: '修复 Bug',
    desc: '定位并修复问题',
    prompt: '请帮我修复以下问题：',
  },
  {
    id: 'code-review',
    icon: '🔍',
    title: '代码审查',
    desc: '审查代码质量和安全',
    prompt: '请审查以下代码变更，关注正确性、安全性和可维护性：',
  },
  {
    id: 'refactor',
    icon: '♻️',
    title: '代码重构',
    desc: '改善代码结构',
    prompt: '请帮我重构以下代码，改善结构和可读性：',
  },
  {
    id: 'optimize',
    icon: '⚡',
    title: '性能优化',
    desc: '优化性能瓶颈',
    prompt: '请帮我分析并优化以下代码的性能：',
  },
  {
    id: 'test',
    icon: '🧪',
    title: '编写测试',
    desc: '添加单元/集成测试',
    prompt: '请为以下代码编写测试：',
  },
  {
    id: 'security',
    icon: '🛡️',
    title: '安全审计',
    desc: '扫描安全漏洞',
    prompt: '请对以下代码进行安全审计：',
  },
  {
    id: 'document',
    icon: '📝',
    title: '编写文档',
    desc: '生成 API/使用文档',
    prompt: '请为以下代码生成文档：',
  },
];

interface TaskTemplatesProps {
  onSelect?: (template: TaskTemplate) => void;
}

export function TaskTemplates({ onSelect }: TaskTemplatesProps) {
  return (
    <div className="task-templates">
      {TEMPLATES.map(template => (
        <div
          key={template.id}
          className="task-template-card"
          onClick={() => onSelect?.(template)}
          title={template.desc}
        >
          <span className="task-template-icon">{template.icon}</span>
          <span className="task-template-title">{template.title}</span>
          <span className="task-template-desc">{template.desc}</span>
        </div>
      ))}
    </div>
  );
}

export { TEMPLATES };
