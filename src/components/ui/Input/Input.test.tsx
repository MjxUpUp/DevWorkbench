import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Input, Textarea, Label } from './Input';
import styles from './Input.module.css';

describe('Input primitive', () => {
  it('Input renders and accepts typed text', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Input placeholder="姓名" onChange={onChange} />);
    const el = screen.getByPlaceholderText('姓名') as HTMLInputElement;
    await user.type(el, 'abc');
    expect(onChange).toHaveBeenCalled();
  });

  it('Input forwards ref', () => {
    let ref: HTMLInputElement | null = null;
    render(<Input ref={(el) => { ref = el; }} />);
    expect(ref).toBeInstanceOf(HTMLInputElement);
  });

  it('Input invalid sets aria-invalid + invalid class', () => {
    render(<Input invalid placeholder="x" />);
    const el = screen.getByPlaceholderText('x');
    expect(el).toHaveAttribute('aria-invalid', 'true');
    expect(el).toHaveClass(styles.invalid);
  });

  it('Input without invalid has no aria-invalid', () => {
    render(<Input placeholder="x" />);
    expect(screen.getByPlaceholderText('x')).not.toHaveAttribute('aria-invalid');
  });

  it('Input forwards native attributes (type, disabled, data-testid)', () => {
    render(<Input type="email" disabled data-testid="email" />);
    const el = screen.getByTestId('email') as HTMLInputElement;
    expect(el).toHaveAttribute('type', 'email');
    expect(el).toBeDisabled();
  });

  it('Textarea renders textarea element', () => {
    render(<Textarea placeholder="留言" />);
    const el = screen.getByPlaceholderText('留言');
    expect(el.tagName).toBe('TEXTAREA');
  });

  it('Textarea invalid sets aria-invalid', () => {
    render(<Textarea invalid placeholder="x" />);
    expect(screen.getByPlaceholderText('x')).toHaveAttribute('aria-invalid', 'true');
  });

  it('Label renders label element with children', () => {
    render(<Label>项目名称</Label>);
    const el = screen.getByText('项目名称');
    expect(el.tagName).toBe('LABEL');
    expect(el).toHaveClass(styles.label);
  });

  it('Label htmlFor associates with Input', () => {
    render(
      <>
        <Label htmlFor="name">名称</Label>
        <Input id="name" />
      </>,
    );
    const label = screen.getByText('名称');
    expect(label).toHaveAttribute('for', 'name');
    expect(document.getElementById('name')).toBeInstanceOf(HTMLInputElement);
  });
});
