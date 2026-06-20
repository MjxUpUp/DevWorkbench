import { useCallback, useEffect, useRef } from 'react';

/**
 * Trailing-edge debounce of a callback. Returns a stable function that, when
 * called repeatedly within `delay` ms, only fires `fn` once — with the LAST
 * invocation's arguments — after the burst settles.
 *
 * Used to coalesce rapid calls (e.g. a settings save on every keystroke) into a
 * single IPC write. The latest-args-wins semantics matter: each call rebuilds
 * the patch from the current render, so the deferred fire carries the final
 * value the user typed, not an intermediate one.
 */
export function useDebouncedCallback<A extends unknown[]>(
  fn: (...args: A) => void,
  delay: number,
): (...args: A) => void {
  const fnRef = useRef(fn);
  useEffect(() => {
    fnRef.current = fn;
  }, [fn]);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  return useCallback(
    (...args: A) => {
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => fnRef.current(...args), delay);
    },
    [delay],
  );
}
