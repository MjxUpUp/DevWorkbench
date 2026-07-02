import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ChatHeader } from '../ChatHeader';

// Stub the sibling selectors so the test isolates the agent <select> only.
vi.mock('../../ModeSelector', () => ({
  ModeSelector: () => <div data-testid="mode-stub" />,
}));
vi.mock('../../ModelSelector', () => ({
  ModelSelector: () => <div data-testid="model-stub" />,
}));

const baseProps = {
  agentMode: 'default' as never,
  onModeChange: () => {},
  selectedModel: 'default',
  onModelChange: () => {},
  onClear: () => {},
};

// 砍 CLI（用户决定 1）：chat 唯一执行路径 = 自研 ReactKernel。ChatHeader 不再
// 从 useAgentStore 读 installed CLI agents 渲染 <option>——CLI agent 选项移除，
// 多模型靠协议层（Anthropic/OpenAI）支撑。select 恒为 Kernel Agent 单选。
describe('ChatHeader — agent selector（砍 CLI 后唯一 Kernel Agent）', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('只渲染 Kernel Agent 单选——store 即便有 installed CLI 也不列', () => {
    render(<ChatHeader selectedAgent="react_kernel" onAgentChange={() => {}} {...baseProps} />);
    const labels = screen.getAllByRole('option').map((o) => o.textContent);
    expect(labels).toEqual(['Kernel Agent']);
  });

  it('selectedAgent=null → select 默认落在唯一 Kernel Agent option', () => {
    // 单 option select：value=null 时浏览器默认选第一个（且唯一）option，不会真
    // 空——与 ChatView 初始化逻辑一致（useEffect 立即 setSelectedAgent('react_kernel')）。
    render(<ChatHeader selectedAgent={null} onAgentChange={() => {}} {...baseProps} />);
    const select = screen.getByRole('combobox') as HTMLSelectElement;
    expect(select.value).toBe('react_kernel');
  });

  it('选 Kernel Agent → onAgentChange(react_kernel)', async () => {
    const user = userEvent.setup();
    const onAgentChange = vi.fn();
    render(<ChatHeader selectedAgent={null} onAgentChange={onAgentChange} {...baseProps} />);
    await user.selectOptions(screen.getByRole('combobox'), 'react_kernel');
    expect(onAgentChange).toHaveBeenCalledWith('react_kernel');
  });
});
