import { describe, it, expect, afterEach } from 'vitest';
import { isTauri } from '../env';

describe('isTauri', () => {
  afterEach(() => {
    // jsdom has no Tauri global by default; ensure cleanup if a test added it
    // @ts-expect-error — intentionally touching the Tauri internal global
    delete window.__TAURI_INTERNALS__;
  });

  it('returns false in a plain browser (no __TAURI_INTERNALS__)', () => {
    // @ts-expect-error — ensure absence
    delete window.__TAURI_INTERNALS__;
    expect(isTauri()).toBe(false);
  });

  it('returns true when __TAURI_INTERNALS__ is present (Tauri webview)', () => {
    // @ts-expect-error — simulate Tauri injection
    window.__TAURI_INTERNALS__ = { invoke: () => Promise.resolve() };
    expect(isTauri()).toBe(true);
  });
});
