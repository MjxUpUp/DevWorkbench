import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { HooksSection } from '../settings/HooksSection';
import { ToastProvider } from '../Toast';
import type { UserHook } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

const SUBMIT_HOOK: UserHook = {
  id: 'h1',
  name: 'load-conventions',
  event: 'user_prompt_submit',
  command: 'cat .cursorrules',
  shell: true,
  timeoutSecs: 30,
  enabled: true,
  createdAt: '2026-06-01',
};
const STOP_HOOK: UserHook = {
  id: 'h2',
  name: 'notify',
  event: 'stop',
  command: 'echo done',
  shell: true,
  timeoutSecs: 10,
  enabled: false,
  createdAt: '2026-06-02',
};

function renderSection() {
  return render(
    <ToastProvider>
      <HooksSection />
    </ToastProvider>,
  );
}

describe('HooksSection', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    vi.mocked(invoke).mockImplementation((cmd, _args) => {
      if (String(cmd) === 'list_user_hooks') {
        return Promise.resolve([SUBMIT_HOOK, STOP_HOOK]);
      }
      if (String(cmd) === 'create_user_hook') {
        return Promise.resolve({ ...SUBMIT_HOOK, id: 'new' });
      }
      return Promise.resolve(null);
    });
  });

  it('lists hooks with event labels and enabled state', async () => {
    renderSection();
    expect(await screen.findByText('load-conventions')).toBeInTheDocument();
    expect(screen.getByText('notify')).toBeInTheDocument();
    // Event labels render on the card badges (提交时 / 停止时). Scoped to the
    // .memory-card-category span via an exact-match function so the description
    // paragraph — which also contains those words — doesn't double-match.
    const badge = (label: string) =>
      screen.getByText((content, el) => {
        const span = el?.tagName === 'SPAN' ? el : null;
        return span?.classList.contains('memory-card-category') === true && content === label;
      });
    expect(badge('提交时')).toBeInTheDocument();
    expect(badge('停止时')).toBeInTheDocument();
    // Enabled toggle reflects state.
    expect(screen.getByLabelText('切换钩子 load-conventions 启用状态').textContent).toContain('已启用');
    expect(screen.getByLabelText('切换钩子 notify 启用状态').textContent).toContain('已禁用');
  });

  it('creates a hook via the form with default event user_prompt_submit', async () => {
    renderSection();
    await screen.findByText('load-conventions');
    fireEvent.click(screen.getByText('+ 新建钩子'));
    fireEvent.change(screen.getByPlaceholderText('例如 load-conventions'), {
      target: { value: 'lint' },
    });
    fireEvent.change(screen.getByPlaceholderText('cat .cursorrules 2>/dev/null || echo 无项目规则'), {
      target: { value: 'echo rule' },
    });
    fireEvent.click(screen.getByText('保存'));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('create_user_hook', {
        name: 'lint',
        event: 'user_prompt_submit', // default form value
        command: 'echo rule',
        shell: true,
        timeoutSecs: 30,
        enabled: true,
      });
    });
  });

  it('refuses an empty name or command', async () => {
    renderSection();
    await screen.findByText('load-conventions');
    fireEvent.click(screen.getByText('+ 新建钩子'));
    // Save with the form untouched (empty name + empty command).
    fireEvent.click(screen.getByText('保存'));
    expect(invoke).not.toHaveBeenCalledWith('create_user_hook', expect.anything());
  });

  it('toggles enabled via the list-card button', async () => {
    renderSection();
    await screen.findByText('load-conventions');
    fireEvent.click(screen.getByLabelText('切换钩子 load-conventions 启用状态'));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('set_user_hook_enabled', { id: 'h1', enabled: false });
    });
  });

  it('deletes a hook after confirmation', async () => {
    renderSection();
    await screen.findByText('load-conventions');
    fireEvent.click(screen.getByLabelText('删除钩子 load-conventions'));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('delete_user_hook', { id: 'h1' });
    });
  });

  it('loads an existing hook into the form on edit', async () => {
    renderSection();
    await screen.findByText('load-conventions');
    fireEvent.click(screen.getByLabelText('编辑钩子 load-conventions'));
    // The form is pre-filled with the hook's name + command + event.
    expect(await screen.findByDisplayValue('load-conventions')).toBeInTheDocument();
    expect(screen.getByDisplayValue('cat .cursorrules')).toBeInTheDocument();
    expect(screen.getByText('编辑钩子')).toBeInTheDocument();
    // Event dropdown holds the stored value.
    expect((screen.getByDisplayValue('提交时（stdout 注入上下文）') as HTMLSelectElement).value).toBe(
      'user_prompt_submit',
    );
  });
});
