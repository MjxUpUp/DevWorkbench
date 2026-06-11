/**
 * Structured output types for PTY byte streams.
 * Recognizes 6 common patterns in agent terminal output:
 * 1. Command execution ($ prefix)
 * 2. Diff blocks (+/-/@@)
 * 3. File paths
 * 4. Progress markers (✓✗✔□)
 * 5. Tool call trees (├─└─│)
 * 6. Plain text (fallback)
 */

export interface ParsedBlock {
  type: 'command' | 'diff' | 'filepath' | 'progress' | 'tool_tree' | 'text';
  content: string;
  meta?: {
    exitCode?: number;
    filePath?: string;
    additions?: number;
    deletions?: number;
    status?: 'running' | 'done' | 'failed';
    duration?: string;
  };
  children?: ParsedBlock[];
}
