import { defineConfig, devices } from '@playwright/test';
import { fileURLToPath } from 'node:url';
import { dirname } from 'node:path';

// __dirname isn't defined under ESM (this project ships `"type": "module"`),
// and unlike vite.config.ts the Playwright config loader doesn't inject it —
// derive the config file's own dir from import.meta.url instead.
const here = dirname(fileURLToPath(import.meta.url));

// E2E harness Playwright config.
//
// The harness lives in this e2e/ dir with its OWN Vite root (vite.config.ts,
// port 5174 strictPort) — separate from the app's vite.config.ts so it can't
// perturb the app build. This config boots that server and sets baseURL so spec
// files can page.goto('/trigger.html') etc. Only the Tauri IPC boundary is
// mocked (window.__MOCK_INVOKE__); real production components run in a real
// browser. Single worker, no parallelism — specs are small and the harness
// server is tiny, and serial avoids port/context races during webServer boot.
export default defineConfig({
  testDir: '.',
  workers: 1,
  fullyParallel: false,
  reporter: 'list',
  timeout: 30_000,
  use: {
    baseURL: 'http://localhost:5174',
    trace: 'retain-on-failure',
  },
  webServer: {
    command: 'npx vite',
    url: 'http://localhost:5174',
    cwd: here,
    reuseExistingServer: true,
    timeout: 60_000,
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],
});
