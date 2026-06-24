import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    // e2e/ holds Playwright specs (Playwright's test() API, not vitest's);
    // .forge/quarantine/ holds file-sentinel source backups (incl. .test/.spec
    // copies) — both must be excluded so `vitest run` only collects the real
    // unit tests under src/.
    exclude: ['**/node_modules/**', '**/dist/**', 'e2e/**', '.forge/**'],
  },
})
