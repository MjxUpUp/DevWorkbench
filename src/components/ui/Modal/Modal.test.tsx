import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Modal } from './Modal';

describe('Modal primitive', () => {
  it('renders children when open', () => {
    render(
      <Modal open onClose={() => {}} aria-label="测试">
        内容
      </Modal>,
    );
    expect(screen.getByText('内容')).toBeInTheDocument();
  });

  it('returns null when closed', () => {
    const { container } = render(
      <Modal open={false} onClose={() => {}} aria-label="测试">
        内容
      </Modal>,
    );
    expect(container.firstChild).toBeNull();
  });

  it('sets role=dialog + aria-modal', () => {
    render(
      <Modal open onClose={() => {}} aria-label="添加项目">
        x
      </Modal>,
    );
    const dialog = screen.getByRole('dialog');
    expect(dialog).toHaveAttribute('aria-modal', 'true');
    expect(dialog).toHaveAttribute('aria-label', '添加项目');
  });

  it('Esc closes the modal', () => {
    const onClose = vi.fn();
    render(
      <Modal open onClose={onClose}>
        x
      </Modal>,
    );
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('Esc does nothing when closed (listener not attached)', () => {
    const onClose = vi.fn();
    render(
      <Modal open={false} onClose={onClose}>
        x
      </Modal>,
    );
    fireEvent.keyDown(window, { key: 'Escape' });
    expect(onClose).not.toHaveBeenCalled();
  });

  it('overlay click closes', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    const { container } = render(
      <Modal open onClose={onClose}>
        内容
      </Modal>,
    );
    // overlay 是第一个子元素（fixed 全屏）
    const overlay = container.firstElementChild as HTMLElement;
    await user.click(overlay);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('content click does NOT close (stopPropagation)', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <Modal open onClose={onClose}>
        <button>内部按钮</button>
      </Modal>,
    );
    await user.click(screen.getByText('内部按钮'));
    expect(onClose).not.toHaveBeenCalled();
  });

  it('width prop sets inline style', () => {
    render(
      <Modal open onClose={() => {}} width={720}>
        x
      </Modal>,
    );
    expect(screen.getByRole('dialog')).toHaveStyle({ width: '720px' });
  });

  it('Modal.Header / Body / Close render', () => {
    render(
      <Modal open onClose={() => {}} aria-label="t">
        <Modal.Header>
          <h2>标题</h2>
          <Modal.Close onClose={() => {}} />
        </Modal.Header>
        <Modal.Body>正文</Modal.Body>
      </Modal>,
    );
    expect(screen.getByText('标题')).toBeInTheDocument();
    expect(screen.getByText('正文')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '关闭' })).toBeInTheDocument();
  });

  it('Modal.Close fires onClose', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(
      <Modal open onClose={() => {}}>
        <Modal.Close onClose={onClose} />
      </Modal>,
    );
    await user.click(screen.getByRole('button', { name: '关闭' }));
    expect(onClose).toHaveBeenCalledTimes(1);
  });
});
