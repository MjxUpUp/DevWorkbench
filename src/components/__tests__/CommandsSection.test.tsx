import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { invoke } from '@tauri-apps/api/core';
import { CommandsSection } from '../settings/CommandsSection';
import { ToastProvider } from '../Toast';
import type { SlashCommand } from '../../types';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(),
}));

const BUILTINS: SlashCommand[] = [
  { id: 'b1', name: 'plan', description: '计划', template: '计划：$ARGUMENTS', category: 'builtin', createdAt: '2026-01-01' },
];
const USER_CMD: SlashCommand = {
  id: 'u1',
  name: 'myreview',
  description: '我的审查',
  template: '审查 $ARGUMENTS',
  category: 'user',
  createdAt: '2026-06-01',
};

function renderSection() {
  return render(
    <ToastProvider>
      <CommandsSection />
    </ToastProvider>,
  );
}

describe('CommandsSection', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    vi.mocked(invoke).mockImplementation((cmd, _args) => {
      if (String(cmd) === 'list_slash_commands') {
        return Promise.resolve([...BUILTINS, USER_CMD]);
      }
      if (String(cmd) === 'create_slash_command') {
        return Promise.resolve({ ...USER_CMD, id: 'new' });
      }
      return Promise.resolve(null);
    });
  });

  it('lists commands and protects builtins from edit/delete', async () => {
    renderSection();
    // Both render with a leading slash.
    expect(await screen.findByText('/plan')).toBeInTheDocument();
    expect(screen.getByText('/myreview')).toBeInTheDocument();
    // Builtin shows the read-only note and has NO 编辑/删除 buttons.
    expect(screen.getByText('内置命令不可编辑')).toBeInTheDocument();
    expect(screen.queryByLabelText('编辑命令 plan')).not.toBeInTheDocument();
    // User command exposes edit + delete.
    expect(screen.getByLabelText('编辑命令 myreview')).toBeInTheDocument();
    expect(screen.getByLabelText('删除命令 myreview')).toBeInTheDocument();
  });

  it('creates a command via the form, stripping a leading slash', async () => {
    renderSection();
    await screen.findByText('/myreview');
    fireEvent.click(screen.getByText('+ 新建命令'));
    fireEvent.change(screen.getByPlaceholderText('例如 myreview'), { target: { value: '/lint' } });
    fireEvent.change(screen.getByPlaceholderText('审查以下代码并指出问题：$ARGUMENTS'), {
      target: { value: 'lint: $ARGUMENTS' },
    });
    fireEvent.click(screen.getByText('保存'));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('create_slash_command', {
        name: 'lint', // leading slash stripped
        description: null, // empty → null
        template: 'lint: $ARGUMENTS',
        category: 'user', // default form value
      });
    });
  });

  it('refuses an empty name or template', async () => {
    renderSection();
    await screen.findByText('/myreview');
    fireEvent.click(screen.getByText('+ 新建命令'));
    // Save with the form untouched (empty name + empty template).
    fireEvent.click(screen.getByText('保存'));
    // Neither create nor update was invoked.
    expect(invoke).not.toHaveBeenCalledWith('create_slash_command', expect.anything());
    expect(invoke).not.toHaveBeenCalledWith('update_slash_command', expect.anything());
  });

  it('deletes a user command after confirmation', async () => {
    renderSection();
    await screen.findByText('/myreview');
    fireEvent.click(screen.getByLabelText('删除命令 myreview'));
    await waitFor(() => {
      expect(invoke).toHaveBeenCalledWith('delete_slash_command', { id: 'u1' });
    });
  });

  it('loads an existing user command into the form on edit', async () => {
    renderSection();
    await screen.findByText('/myreview');
    fireEvent.click(screen.getByLabelText('编辑命令 myreview'));
    // The form is pre-filled with the command's name (no leading slash) + template.
    expect(await screen.findByDisplayValue('myreview')).toBeInTheDocument();
    expect(screen.getByDisplayValue('审查 $ARGUMENTS')).toBeInTheDocument();
    expect(screen.getByText('编辑命令')).toBeInTheDocument();
  });
});
