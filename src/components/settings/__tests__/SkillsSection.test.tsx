import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { SkillsSection } from '../SkillsSection';
import { useSkillsStore } from '../../../stores/skillsStore';
import { useNavigationStore } from '../../../stores/navigationStore';
import type { Skill, SkillCatalogEntry } from '../../../types';

/**
 * SkillsSection surfaces the already-built skills subsystem (list_skills /
 * skill_catalog / install_skill_from_catalog / uninstall_skill). These tests
 * stub invoke by command name + Toast and drive the stores directly. Covers:
 * load on mount, install from catalog round-trip, uninstall round-trip.
 */
const mockInvoke = vi.hoisted(() => vi.fn());
const toastSpies = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));
vi.mock('../../Toast', () => ({ useToast: () => toastSpies }));

function makeSkill(id: string, name: string): Skill {
  return {
    id,
    org: 'local',
    name,
    description: `${name} desc`,
    category: 'tool',
    rating: 4.5,
    installedAt: '2026-06-17T00:00:00Z',
  };
}
function makeCatalog(name: string, scope: 'global' | 'project' = 'global'): SkillCatalogEntry {
  return {
    name,
    description: `${name} catalog`,
    source: `/home/.agents/skills/${name}`,
    scope,
  };
}

describe('SkillsSection', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    Object.values(toastSpies).forEach((s) => s.mockClear());
    useSkillsStore.setState({ installed: [], catalog: [], loading: false });
    useNavigationStore.setState({ activeProject: null });
  });

  it('loads installed skills and the catalog on mount', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'list_skills') return Promise.resolve([makeSkill('s1', 'my-skill')]);
      if (cmd === 'skill_catalog') return Promise.resolve([makeCatalog('discoverable')]);
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<SkillsSection />);

    expect(await screen.findByText('my-skill')).toBeInTheDocument();
    expect(await screen.findByText('discoverable')).toBeInTheDocument();
  });

  it('installs a catalog skill via install_skill_from_catalog', async () => {
    const installed = makeSkill('s1', 'my-skill');
    const fresh = makeSkill('s2', 'fresh-skill');
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === 'list_skills') return Promise.resolve([installed]);
      if (cmd === 'skill_catalog') return Promise.resolve([makeCatalog('fresh-skill')]);
      if (cmd === 'install_skill_from_catalog') {
        expect(args).toEqual({
          name: 'fresh-skill',
          source: '/home/.agents/skills/fresh-skill',
        });
        return Promise.resolve(fresh);
      }
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<SkillsSection />);
    await screen.findByText('my-skill');

    fireEvent.click(screen.getByRole('button', { name: '安装技能 fresh-skill' }));

    await waitFor(() =>
      expect(toastSpies.success).toHaveBeenCalledWith('已安装技能 fresh-skill'),
    );
    expect(mockInvoke).toHaveBeenCalledWith('install_skill_from_catalog', {
      name: 'fresh-skill',
      source: '/home/.agents/skills/fresh-skill',
    });
    // After install, fresh-skill appears both in the catalog card and the
    // installed list.
    expect(await screen.findAllByText('fresh-skill')).toHaveLength(2);
  });

  it('uninstalls via uninstall_skill and removes the card', async () => {
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === 'list_skills') return Promise.resolve([makeSkill('s1', 'drop-me')]);
      if (cmd === 'skill_catalog') return Promise.resolve([]);
      if (cmd === 'uninstall_skill') {
        expect(args).toEqual({ id: 's1' });
        return Promise.resolve();
      }
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<SkillsSection />);
    await screen.findByText('drop-me');

    fireEvent.click(screen.getByRole('button', { name: '卸载技能 drop-me' }));

    await waitFor(() =>
      expect(toastSpies.success).toHaveBeenCalledWith('已卸载技能 drop-me'),
    );
    expect(mockInvoke).toHaveBeenCalledWith('uninstall_skill', { id: 's1' });
    expect(screen.queryByText('drop-me')).not.toBeInTheDocument();
  });
});
