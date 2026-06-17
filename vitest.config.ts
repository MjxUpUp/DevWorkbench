import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    // e2e/ holds Playwright specs (Playwright's test() API, not vitest's);
    // exclude it so `vitest run` only collects the real unit tests under src/.
    exclude: ['**/node_modules/**', '**/dist/**', 'e2e/**'],
  },
})
