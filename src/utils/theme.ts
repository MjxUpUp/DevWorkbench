/**
 * Theme application — three-state (light / dark / auto).
 *
 * `auto` follows the OS via `prefers-color-scheme` and live-updates when the
 * system theme changes. The CSS in variables.css drives the actual colors:
 * `:root` holds light tokens, `[data-theme="dark"]` overrides them. We always
 * write an explicit `data-theme` attribute so the DOM state is unambiguous
 * (a `data-theme="light"` attribute is harmless — the `:root` light rules
 * apply regardless of the attribute value).
 */

export type Theme = 'light' | 'dark' | 'auto';

/** The theme actually rendered right now (resolved from auto if needed). */
export type ResolvedTheme = 'light' | 'dark';

let mediaListener: ((e: MediaQueryListEvent) => void) | null = null;
let trackedMedia: MediaQueryList | null = null;

function systemPrefersDark(): boolean {
  return typeof window !== 'undefined'
    && typeof window.matchMedia === 'function'
    && window.matchMedia('(prefers-color-scheme: dark)').matches;
}

function writeAttr(theme: ResolvedTheme) {
  document.documentElement.setAttribute('data-theme', theme);
}

/**
 * Apply a theme. For `auto`, subscribes to system changes and re-applies.
 * Calling again with a different theme cleans up the previous subscription.
 */
export function applyTheme(theme: Theme): ResolvedTheme {
  // Tear down any existing auto-subscription first.
  if (mediaListener && trackedMedia) {
    trackedMedia.removeEventListener('change', mediaListener);
    mediaListener = null;
    trackedMedia = null;
  }

  if (theme === 'auto') {
    const media = window.matchMedia('(prefers-color-scheme: dark)');
    trackedMedia = media;
    mediaListener = (e: MediaQueryListEvent) => writeAttr(e.matches ? 'dark' : 'light');
    media.addEventListener('change', mediaListener);
    const resolved: ResolvedTheme = media.matches ? 'dark' : 'light';
    writeAttr(resolved);
    return resolved;
  }

  writeAttr(theme);
  return theme;
}

/** Read the persisted theme string back into the safe union. */
export function normalizeTheme(value: unknown): Theme {
  return value === 'dark' || value === 'auto' ? value : 'light';
}

/** What is being shown right now, regardless of the chosen mode. */
export function resolvedTheme(): ResolvedTheme {
  return document.documentElement.getAttribute('data-theme') === 'dark' ? 'dark' : 'light';
}

/** Whether the OS currently prefers dark (used to highlight the Auto choice). */
export function systemIsDark(): boolean {
  return systemPrefersDark();
}
