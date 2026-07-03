// Type declarations for the .mjs build-time helper. The helper itself is plain
// ESM JavaScript (no types); vite.config.ts imports it to regenerate the P4
// IPC-coverage manifest at build/dev time.
export function collectInvokedCommands(): string[];
export function generateInvokedCommandsFile(): number;
