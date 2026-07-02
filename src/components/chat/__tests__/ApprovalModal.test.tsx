import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(() => Promise.resolve(() => {})) }));

import { invoke } from '@tauri-apps/api/core';
import { useAgentStore } from '../../../stores/agentStore';
import { ApprovalModal } from '../ApprovalModal';

const PENDING = {
  sessionId: 'sess-1',
  tool: 'bash',
  arguments: JSON.stringify({ command: 'rm -rf build/' }),
  resumeToken: 'approve__sess-1__0',
  summary: '即将执行破坏性命令：rm -rf build/',
};

function setPending(p: typeof PENDING | null) {
  useAgentStore.setState({ pendingApproval: p } as never);
}

describe('ApprovalModal (Human Gate)', () => {
  beforeEach(() => {
    vi.mocked(invoke).mockReset();
    vi.mocked(invoke).mockResolvedValue(undefined as never);
    setPending(PENDING);
  });

  it('renders nothing when no approval is pending', () => {
    setPending(null);
    const { container } = render(<ApprovalModal />);
    expect(container.firstChild).toBeNull();
  });

  it('shows the destructive summary + tool + pretty-printed args', () => {
    render(<ApprovalModal />);
    expect(screen.getByText(PENDING.summary)).toBeInTheDocument();
    expect(screen.getByText('bash')).toBeInTheDocument();
    // JSON args pretty-printed (indented) in the preview <pre>, scoped by label
    // so the title's copy of the command doesn't double-match.
    const preview = screen.getByLabelText('操作参数预览');
    expect(preview.textContent).toMatch(/rm -rf build\//);
    expect(preview.textContent).toMatch(/"/); // pretty-printed JSON has quotes
  });

  it('Approve resolves with action=approve', async () => {
    render(<ApprovalModal />);
    fireEvent.click(screen.getByText('同意执行'));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('resolve_human_gate_cmd', {
      resumeToken: PENDING.resumeToken,
      action: 'approve',
      feedback: null,
    });
  });

  it('Reject resolves with action=reject', async () => {
    render(<ApprovalModal />);
    fireEvent.click(screen.getByText('拒绝（阻止执行）'));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('resolve_human_gate_cmd', {
      resumeToken: PENDING.resumeToken,
      action: 'reject',
      feedback: null,
    });
  });

  it('Retry reveals a textarea and sends the feedback', async () => {
    render(<ApprovalModal />);
    fireEvent.click(screen.getByText('重试（补充指令）'));
    const ta = await screen.findByPlaceholderText(/补充给 Agent 的指令/);
    fireEvent.change(ta, { target: { value: '换个目录再删' } });
    fireEvent.click(screen.getByText('发送重试指令'));
    expect(vi.mocked(invoke)).toHaveBeenCalledWith('resolve_human_gate_cmd', {
      resumeToken: PENDING.resumeToken,
      action: 'retry',
      feedback: '换个目录再删',
    });
  });

  it('Retry is disabled until feedback is non-empty', async () => {
    render(<ApprovalModal />);
    fireEvent.click(screen.getByText('重试（补充指令）'));
    const send = await screen.findByText('发送重试指令');
    expect((send as HTMLButtonElement).disabled).toBe(true);
  });
});
