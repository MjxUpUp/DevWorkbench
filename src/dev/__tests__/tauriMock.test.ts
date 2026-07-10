import { describe, it, expect } from 'vitest';
import { handlers } from '../tauriMock';

/**
 * The dev mock must faithfully match the Rust command contracts — otherwise the
 * plain-browser UI (and Playwright harness) renders against wrong shapes and
 * hides real bugs. detect_tools now returns only the non-agent tools (code/git)
 * after retiring the external CLI agent link; the agent command_name loop was
 * removed from the backend's detect_tools. Pin that shape so it can't drift.
 */

describe('dev mock contract fidelity', () => {
  it('detect_tools returns a ToolStatus[] array with only code/git (no agent tools)', () => {
    const tools = handlers.detect_tools({}) as Array<Record<string, unknown>>;
    expect(Array.isArray(tools)).toBe(true);
    expect(tools.length).toBeGreaterThan(0);

    for (const t of tools) {
      expect(typeof t.name).toBe('string');
      expect(typeof t.installed).toBe('boolean');
    }

    // code + git must be present — the only non-agent tools detect_tools tests.
    const names = tools.map((t) => t.name as string);
    expect(names).toContain('code');
    expect(names).toContain('git');
    // Agent command names (claude/codex/…) are no longer surfaced here.
    expect(names).not.toContain('claude');
    expect(names).not.toContain('codex');
  });
});
