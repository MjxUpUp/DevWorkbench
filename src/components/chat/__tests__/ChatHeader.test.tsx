import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ChatHeader } from '../ChatHeader';

// Stub ModelSelector so the test isolates ChatHeader's own chrome (requestId /
// LiveDot / clear). 模式选择器与 agent 选择器已移除，不再需要 stub ModeSelector。
vi.mock('../../ModelSelector', () => ({
  ModelSelector: () => <div data-testid="model-stub" />,
}));

// 砍 CLI + 移除模式/agent 选择器后，ChatHeader 只剩 ModelSelector（保留）+ requestId/
// LiveDot + 清空。这些测试钉住保留侧不被误删。
describe('ChatHeader — 模型选择器保留（agent/模式选择器已移除）', () => {
  it('渲染 ModelSelector（模型选择器保留）', () => {
    render(<ChatHeader selectedModel="default" onModelChange={() => {}} onClear={() => {}} />);
    expect(screen.getByTestId('model-stub')).toBeInTheDocument();
  });

  it('点击清空按钮 → onClear', async () => {
    const user = userEvent.setup();
    const onClear = vi.fn();
    render(<ChatHeader selectedModel="default" onModelChange={() => {}} onClear={onClear} />);
    await user.click(screen.getByTitle('清空对话'));
    expect(onClear).toHaveBeenCalled();
  });

  it('running=true → 显示 LiveDot + requestId', () => {
    render(<ChatHeader selectedModel="default" onModelChange={() => {}} onClear={() => {}} running requestId="req-1" />);
    expect(screen.getByText('req-1')).toBeInTheDocument();
    expect(document.querySelector('.chat-header-livedot')).not.toBeNull();
  });
});
