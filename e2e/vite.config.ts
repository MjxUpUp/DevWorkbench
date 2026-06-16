import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// Dedicated Vite config for the E2E harness. `root` is this e2e/ dir so the
// harness index.html is served at '/', and it resolves the real production
// FileChanges via the relative path into ../src. Kept separate from the app's
// vite.config.ts so it can't perturb the app build.
export default defineConfig({
  root: __dirname,
  server: { port: 5174, strictPort: true },
  plugins: [react()],
});
