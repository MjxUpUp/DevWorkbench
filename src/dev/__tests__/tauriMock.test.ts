import { describe, it, expect } from 'vitest';
import { handlers } from '../tauriMock';

/**
 * The dev mock must faithfully match the Rust command contracts — otherwise the
 * plain-browser UI (and Playwright harness) renders against wrong shapes and
 * hides real bugs. This regression guard exists because `discover_agents_cmd`
 * previously returned AgentInfo objects WITHOUT `commandName`, which made
 * AgentSection's `key={a.commandName}` collide (every entry key=undefined →
 * React "unique key" warnings), and `detect_tools` returned an object instead
 * of ToolStatus[] (silently swallowed by the Array.isArray guard). Pin both
 * shapes so they can't drift again.
 */

describe('dev mock contract fidelity', () => {
  it('discover_agents_cmd returns full AgentInfo with unique commandName', () => {
    const agents = handlers.discover_agents_cmd({}) as Array<Record<string, unknown>>;
    expect(agents.length).toBeGreaterThan(0);

    const commandNames: string[] = [];
    for (const a of agents) {
      // commandName is the field AgentSection uses as the React key — must be
      // present AND unique, or the settings page emits key warnings.
      expect(typeof a.commandName).toBe('string');
      expect(a.commandName).not.toBe('');
      expect(a.agentType).toBeTruthy();
      expect(a.displayName).toBeTruthy();
      expect(typeof a.installed).toBe('boolean');
      commandNames.push(a.commandName as string);
    }
    expect(new Set(commandNames).size).toBe(commandNames.length);
  });

  it('detect_tools returns a ToolStatus[] array (not an object)', () => {
    const tools = handlers.detect_tools({}) as Array<Record<string, unknown>>;
    expect(Array.isArray(tools)).toBe(true);
    expect(tools.length).toBeGreaterThan(0);

    for (const t of tools) {
      expect(typeof t.name).toBe('string');
      expect(typeof t.installed).toBe('boolean');
    }

    // code + git must be present — AgentSection's NON_AGENT_TOOLS filter
    // (['code', 'git']) relies on them to populate the non-agent tool cards.
    const names = tools.map((t) => t.name as string);
    expect(names).toContain('code');
    expect(names).toContain('git');
  });
});
