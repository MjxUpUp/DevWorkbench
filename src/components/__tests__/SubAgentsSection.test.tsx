import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { SubAgentsSection } from '../settings/SubAgentsSection';
import { ToastProvider } from '../Toast';
import { useNavigationStore } from '../../stores/navigationStore';
import type { SubAgentInfo } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

const PROJECT_AGENT: SubAgentInfo = {
  name: 'researcher',
  description: 'deep web research',
  systemPrompt: '你是调研专家',
  toolsAllow: ['skill__web_search', 'read_file'],
  scope: 'project',
  sourcePath: '/proj/.agents/subagents/researcher/AGENT.md',
};

const APP_PRIVATE_AGENT: SubAgentInfo = {
  name: 'builtin-helper',
  description: 'shipped with the app',
  systemPrompt: 'internal',
  toolsAllow: [],
  scope: 'app-private',
  sourcePath: '/home/.dev-workbench/subagents/builtin-helper/AGENT.md',
};

function renderSection() {
  return render(
    <ToastProvider>
      <SubAgentsSection />
    </ToastProvider>,
  );
}

describe('SubAgentsSection', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    // Provide an active project so the 'project' scope is writable.
    useNavigationStore.setState({ activeProject: { id: 'p1', name: 'P', path: '/proj' } as never });
    vi.mocked(invoke).mockImplementation((cmd) => {
      if (String(cmd) === 'list_subagents') {
        return Promise.resolve([PROJECT_AGENT]);
      }
      return Promise.resolve(null);
    });
  });

  it('lists sub-agents with scope label, system prompt, tools', async () => {
    renderSection();
    expect(await screen.findByText('researcher')).toBeInTheDocument();
    expect(screen.getByText('deep web research')).toBeInTheDocument();
    expect(screen.getByText('你是调研专家')).toBeInTheDocument();
    // tools line rendered.
    expect(screen.getByText('tools: skill__web_search, read_file')).toBeInTheDocument();
    // project scope label.
    const badge = screen.getByText((content, el) => {
      const span = el?.tagName === 'SPAN' ? el : null;
      return span?.classList.contains('memory-card-category') === true && content === '项目';
    });
    expect(badge).toBeInTheDocument();
  });

  it('creates a sub-agent, parsing comma-separated tools_allow into an array', async () => {
    renderSection();
    await screen.findByText('researcher');
    fireEvent.click(screen.getByText('+ 新建子智能体'));
    fireEvent.change(screen.getByPlaceholderText('例如 researcher'), { target: { value: 'test-writer' } });
    fireEvent.change(screen.getByPlaceholderText('你是调研专家，只给结论不要过程。'), {
      target: { value: '写测试' },
    });
    fireEvent.change(screen.getByPlaceholderText('skill__web_search, read_file'), {
      target: { value: 'read_file, write_file' },
    });
    fireEvent.click(screen.getByText('保存'));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('save_subagent', {
        projectPath: '/proj',
        name: 'test-writer',
        description: '',
        systemPrompt: '写测试',
        toolsAllow: ['read_file', 'write_file'], // comma-split + trimmed
        scope: 'project', // active project present → default project scope
      });
    });
  });

  it('refuses an empty name or system prompt', async () => {
    renderSection();
    await screen.findByText('researcher');
    fireEvent.click(screen.getByText('+ 新建子智能体'));
    fireEvent.click(screen.getByText('保存'));
    expect(invoke).not.toHaveBeenCalledWith('save_subagent', expect.anything());
  });

  it('blanks tools_allow into an empty array when left empty', async () => {
    renderSection();
    await screen.findByText('researcher');
    fireEvent.click(screen.getByText('+ 新建子智能体'));
    fireEvent.change(screen.getByPlaceholderText('例如 researcher'), { target: { value: 'noop' } });
    fireEvent.change(screen.getByPlaceholderText('你是调研专家，只给结论不要过程。'), {
      target: { value: 'do nothing' },
    });
    // tools_allow left blank.
    fireEvent.click(screen.getByText('保存'));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('save_subagent', expect.objectContaining({
        name: 'noop',
        toolsAllow: [],
      }));
    });
  });

  it('deletes a sub-agent after confirmation', async () => {
    renderSection();
    await screen.findByText('researcher');
    fireEvent.click(screen.getByLabelText('删除子智能体 researcher'));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('delete_subagent', {
        projectPath: '/proj',
        name: 'researcher',
        scope: 'project',
      });
    });
  });

  it('treats legacy app-private sub-agents as read-only (no edit/delete)', async () => {
    // app-private (~/.dev-workbench/subagents) is a legacy read-only tier: the
    // kernel loads it for dispatch, but save/delete only resolve global/project.
    // Acting on an app-private row would either fail ("子智能体 X 不存在") or
    // silently shadow it with a project copy — so the card must hide both actions
    // and show a read-only note instead.
    vi.mocked(invoke).mockImplementation((cmd) => {
      if (String(cmd) === 'list_subagents') return Promise.resolve([APP_PRIVATE_AGENT]);
      return Promise.resolve(null);
    });
    renderSection();
    expect(await screen.findByText('builtin-helper')).toBeInTheDocument();
    expect(screen.queryByLabelText('编辑子智能体 builtin-helper')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('删除子智能体 builtin-helper')).not.toBeInTheDocument();
    // The read-only hint is specific (the section desc also contains "只读" in
    // "继承完整只读工具集"), so match the hint's distinct prefix, not bare 只读.
    expect(screen.getByText(/内置\/只读/)).toBeInTheDocument();
  });

  it('loads an existing sub-agent into the form with name disabled', async () => {
    renderSection();
    await screen.findByText('researcher');
    fireEvent.click(screen.getByLabelText('编辑子智能体 researcher'));
    // Pre-filled + name field disabled (rename not allowed).
    expect(await screen.findByDisplayValue('researcher')).toBeDisabled();
    expect(screen.getByDisplayValue('你是调研专家')).toBeInTheDocument();
    expect(screen.getByDisplayValue('skill__web_search, read_file')).toBeInTheDocument();
    expect(screen.getByText('编辑：researcher')).toBeInTheDocument();
  });

  it('refuses project scope when no active project is open', async () => {
    // No active project. openCreate defaults scope to 'global' in that case, so
    // to exercise the guard the user must MANUALLY pick 'project' scope — then
    // the client-side check must refuse the save (the backend would error too,
    // but failing fast avoids a round trip).
    useNavigationStore.setState({ activeProject: null });
    renderSection();
    await screen.findByText('researcher');
    fireEvent.click(screen.getByText('+ 新建子智能体'));
    fireEvent.change(screen.getByPlaceholderText('例如 researcher'), { target: { value: 'x' } });
    fireEvent.change(screen.getByPlaceholderText('你是调研专家，只给结论不要过程。'), {
      target: { value: 'sys' },
    });
    // Manually switch to project scope (no project open).
    fireEvent.change(screen.getByDisplayValue(/项目/), { target: { value: 'project' } });
    fireEvent.click(screen.getByText('保存'));
    expect(invoke).not.toHaveBeenCalledWith('save_subagent', expect.anything());
  });
});
