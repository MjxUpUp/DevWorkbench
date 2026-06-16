import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Composer } from '../Composer';

const noop = vi.fn();
const baseProps = {
  prompt: '',
  onPromptChange: noop,
  onSend: noop,
  onStop: noop,
  canSend: true,
  isRunning: false,
  attachedFiles: [],
  onAttachFile: noop,
  onRemoveFile: noop,
};

describe('Composer', () => {
  it('renders the three explicit trigger buttons (@ / / $)', () => {
    render(<Composer {...baseProps} />);
    expect(screen.getByTitle('附加文件 (@)')).toBeInTheDocument();
    expect(screen.getByTitle('命令 (/)')).toBeInTheDocument();
    expect(screen.getByTitle('技能 ($)')).toBeInTheDocument();
  });

  it('opens the file menu when @ is clicked', async () => {
    const user = userEvent.setup();
    render(<Composer {...baseProps} />);
    await user.click(screen.getByTitle('附加文件 (@)'));
    expect(screen.getByText('附加文件')).toBeInTheDocument();
  });

  it('opens the command menu when / is clicked', async () => {
    const user = userEvent.setup();
    render(<Composer {...baseProps} />);
    await user.click(screen.getByTitle('命令 (/)'));
    expect(screen.getByText('命令')).toBeInTheDocument();
  });

  it('opens the skill menu when $ is clicked', async () => {
    const user = userEvent.setup();
    render(<Composer {...baseProps} />);
    await user.click(screen.getByTitle('技能 ($)'));
    expect(screen.getByText('技能')).toBeInTheDocument();
  });

  it('toggles a trigger menu off when the same trigger is clicked twice', async () => {
    const user = userEvent.setup();
    render(<Composer {...baseProps} />);
    const atBtn = screen.getByTitle('附加文件 (@)');
    await user.click(atBtn);
    expect(screen.getByText('附加文件')).toBeInTheDocument();
    await user.click(atBtn);
    expect(screen.queryByText('附加文件')).not.toBeInTheDocument();
  });

  it('marks the active trigger button with data-active', async () => {
    const user = userEvent.setup();
    render(<Composer {...baseProps} />);
    const cmdBtn = screen.getByTitle('命令 (/)');
    expect(cmdBtn.getAttribute('data-active')).toBeNull();
    await user.click(cmdBtn);
    expect(cmdBtn.getAttribute('data-active')).toBe('true');
  });
});
