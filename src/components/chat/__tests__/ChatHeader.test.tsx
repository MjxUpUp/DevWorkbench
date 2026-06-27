import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ChatHeader } from '../ChatHeader';
import { useAgentStore } from '../../../stores/agentStore';

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

describe('ChatHeader — agent selector', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('renders only installed agents as options and reflects selectedAgent', () => {
    useAgentStore.setState({
      agents: [
        { agentType: 'claude_code', displayName: 'Claude Code', installed: true },
        { agentType: 'codex', displayName: 'Codex', installed: true },
        { agentType: 'gemini', displayName: 'Gemini', installed: false },
      ],
    } as never);

    render(<ChatHeader selectedAgent="codex" onAgentChange={() => {}} {...baseProps} />);

    const select = screen.getByRole('combobox') as HTMLSelectElement;
    expect(select.value).toBe('codex');
    // Gemini is not installed → must not appear as an option. The self-hosted
    // Kernel Agent is always available (no CLI to discover), so it's a
    // hardcoded <option> appended after the installed agents.
    const labels = screen.getAllByRole('option').map((o) => o.textContent);
    expect(labels).toEqual(['Claude Code', 'Codex', 'Kernel Agent']);
  });

  it('shows the empty placeholder when no agent is installed', () => {
    useAgentStore.setState({ agents: [] } as never);

    render(<ChatHeader selectedAgent={null} onAgentChange={() => {}} {...baseProps} />);

    expect(screen.getByText('无可用 Agent')).toBeInTheDocument();
  });

  it('fires onAgentChange with the chosen AgentType', async () => {
    const user = userEvent.setup();
    const onAgentChange = vi.fn();
    useAgentStore.setState({
      agents: [
        { agentType: 'claude_code', displayName: 'Claude Code', installed: true },
        { agentType: 'codex', displayName: 'Codex', installed: true },
      ],
    } as never);

    render(<ChatHeader selectedAgent="claude_code" onAgentChange={onAgentChange} {...baseProps} />);

    await user.selectOptions(screen.getByRole('combobox'), 'codex');
    expect(onAgentChange).toHaveBeenCalledWith('codex');
  });
});
