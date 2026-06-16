import { defineConfig, devices } from '@playwright/test';

// Frontend E2E for Tauri/React UI changes. The harness is a standalone Vite
// page that mounts the real production component with Tauri IPC shimmed (see
// e2e/index.html), so Playwright exercises the genuine React render + real
// browser events without spinning up the Rust backend.
export default defineConfig({
  testDir: './e2e',
  fullyParallel: true,
  use: {
    baseURL: 'http://localhost:5174',
    trace: 'on-first-retry',
  },
  projects: [
    { name: 'chromium', use: { ...devices['Desktop Chrome'] } },
  ],
  webServer: {
    command: 'npx vite --config e2e/vite.config.ts',
    url: 'http://localhost:5174',
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
});
