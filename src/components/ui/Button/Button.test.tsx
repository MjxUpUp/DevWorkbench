import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Button } from './Button';
import styles from './Button.module.css';

describe('Button primitive', () => {
  it('renders children', () => {
    render(<Button>保存</Button>);
    expect(screen.getByRole('button', { name: '保存' })).toBeInTheDocument();
  });

  it('defaults to type="button" (避免意外触发表单提交)', () => {
    render(<Button>x</Button>);
    expect(screen.getByRole('button')).toHaveAttribute('type', 'button');
  });

  it('applies variant class', () => {
    const { container } = render(<Button variant="primary">ok</Button>);
    expect(container.firstChild).toHaveClass(styles.primary);
  });

  it('secondary variant adds no extra variant class (base button is secondary)', () => {
    const { container } = render(<Button variant="secondary">ok</Button>);
    // secondary 是默认态，不应附加 primary/ghost/danger 类
    expect(container.firstChild).not.toHaveClass(styles.primary);
    expect(container.firstChild).not.toHaveClass(styles.ghost);
    expect(container.firstChild).not.toHaveClass(styles.danger);
  });

  it('ghost and danger apply their classes', () => {
    const { container: ghostC } = render(<Button variant="ghost">g</Button>);
    expect(ghostC.firstChild).toHaveClass(styles.ghost);
    const { container: dangerC } = render(<Button variant="danger">d</Button>);
    expect(dangerC.firstChild).toHaveClass(styles.danger);
  });

  it('dangerGhost applies class (删除/移除类动作)', () => {
    const { container } = render(<Button variant="dangerGhost">删除</Button>);
    expect(container.firstChild).toHaveClass(styles.dangerGhost);
  });

  it('dashed applies class (添加类动作)', () => {
    const { container } = render(<Button variant="dashed">+ 添加</Button>);
    expect(container.firstChild).toHaveClass(styles.dashed);
  });

  it('size="sm" applies sm class', () => {
    const { container } = render(<Button size="sm">ok</Button>);
    expect(container.firstChild).toHaveClass(styles.sm);
  });

  it('renders leading and trailing icons in aria-hidden spans', () => {
    render(
      <Button leadingIcon={<svg data-testid="lead" />} trailingIcon={<svg data-testid="trail" />}>
        发送
      </Button>,
    );
    expect(screen.getByTestId('lead')).toBeInTheDocument();
    expect(screen.getByTestId('trail')).toBeInTheDocument();
    // icon 容器对屏幕阅读器隐藏，文字仍可读
    expect(screen.getByRole('button', { name: '发送' })).toBeInTheDocument();
  });

  it('fires onClick', async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(<Button onClick={onClick}>click</Button>);
    await user.click(screen.getByRole('button'));
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('disabled blocks onClick', async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(
      <Button disabled onClick={onClick}>
        x
      </Button>,
    );
    await user.click(screen.getByRole('button'));
    expect(onClick).not.toHaveBeenCalled();
    expect(screen.getByRole('button')).toBeDisabled();
  });

  it('isLoading disables + sets aria-busy', () => {
    render(<Button isLoading>x</Button>);
    const btn = screen.getByRole('button');
    expect(btn).toBeDisabled();
    expect(btn).toHaveAttribute('aria-busy', 'true');
  });

  it('forwards native button attributes (data-testid, aria-label)', () => {
    render(
      <Button data-testid="save-btn" aria-label="保存设置">
        保存
      </Button>,
    );
    expect(screen.getByTestId('save-btn')).toBeInTheDocument();
    expect(screen.getByRole('button')).toHaveAttribute('aria-label', '保存设置');
  });

  it('forwards ref', () => {
    let btnRef: HTMLButtonElement | null = null;
    render(<Button ref={(el) => { btnRef = el; }}>x</Button>);
    expect(btnRef).toBeInstanceOf(HTMLButtonElement);
  });
});
