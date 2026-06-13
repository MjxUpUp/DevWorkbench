/**
 * Detect whether the app is running inside the Tauri webview (IPC available)
 * versus a plain browser / `vite dev` preview.
 *
 * Tauri 2 injects `window.__TAURI_INTERNALS__`, which `@tauri-apps/api`'s
 * `invoke` reads. In a plain browser that global is absent, so every `invoke`
 * throws "Cannot read properties of undefined (reading 'invoke')". IPC-backed
 * features (project loading, tool detection, git status, …) cannot work there,
 * so call sites should short-circuit with a graceful empty state instead of
 * surfacing a misleading error banner.
 */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}
