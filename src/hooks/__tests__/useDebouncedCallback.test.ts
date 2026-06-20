import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';
import { useDebouncedCallback } from '../useDebouncedCallback';

describe('useDebouncedCallback — trailing-edge debounce', () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('fires once with the LAST args after the delay', () => {
    const fn = vi.fn();
    const { result } = renderHook(() => useDebouncedCallback(fn, 300));

    act(() => {
      result.current('a');
      result.current('b');
      result.current('c');
    });
    expect(fn).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(299);
    });
    expect(fn).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(1);
    });
    expect(fn).toHaveBeenCalledTimes(1);
    expect(fn).toHaveBeenCalledWith('c');
  });

  it('reschedules on each call — only the trailing burst fires', () => {
    const fn = vi.fn();
    const { result } = renderHook(() => useDebouncedCallback(fn, 100));

    act(() => {
      result.current(1);
    });
    act(() => {
      vi.advanceTimersByTime(90);
    });
    act(() => {
      result.current(2); // resets the window — first call never fires
    });
    act(() => {
      vi.advanceTimersByTime(90);
    });
    expect(fn).not.toHaveBeenCalled();

    act(() => {
      vi.advanceTimersByTime(10);
    });
    expect(fn).toHaveBeenCalledTimes(1);
    expect(fn).toHaveBeenCalledWith(2);
  });

  it('uses the latest fn closure when the passed fn changes', () => {
    let captured = '';
    const { result, rerender } = renderHook(
      ({ cb }) => useDebouncedCallback(cb, 100),
      { initialProps: { cb: (v: string) => { captured = 'first:' + v; } } },
    );
    // fnRef updates on re-render, so the deferred fire runs the NEW callback.
    rerender({ cb: (v: string) => { captured = 'second:' + v; } });

    act(() => {
      result.current('x');
    });
    act(() => {
      vi.advanceTimersByTime(100);
    });
    expect(captured).toBe('second:x');
  });

  it('returns a stable callback identity across re-renders when delay is constant', () => {
    const fn = vi.fn();
    const { result, rerender } = renderHook(() => useDebouncedCallback(fn, 100));
    const first = result.current;
    rerender();
    expect(result.current).toBe(first);
  });
});
