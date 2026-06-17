import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { MemorySection } from '../MemorySection';
import { useKnowledgeStore } from '../../../stores/knowledgeStore';
import { useNavigationStore } from '../../../stores/navigationStore';
import type { KnowledgeEntry } from '../../../types';

/**
 * MemorySection surfaces the v1.3-T2 long-term memory flywheel. These tests
 * stub the Tauri invoke bridge by command name + Toast, and drive the
 * navigation/knowledge stores directly. Covers: project-scoped load on mount,
 * the switch to global search results when typing, and delete round-trip.
 */
const mockInvoke = vi.hoisted(() => vi.fn());
const toastSpies = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));
vi.mock('../../Toast', () => ({ useToast: () => toastSpies }));

const PROJECT_PATH = 'E:\\proj';

function makeEntry(
  id: string,
  title: string,
  category = 'memory',
  confidence = 0.8,
): KnowledgeEntry {
  return {
    id,
    projectHash: 'h',
    category,
    title,
    content: `content for ${title} `.repeat(3),
    sourceAgent: 'react_kernel',
    sourceSessionId: null,
    sourceType: 'react_session',
    confidence,
    createdAt: '2026-06-17T00:00:00Z',
    updatedAt: '2026-06-17T00:00:00Z',
    accessCount: 0,
  };
}

describe('MemorySection', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    Object.values(toastSpies).forEach((s) => s.mockClear());
    useKnowledgeStore.setState({ entries: [], searchResults: [], loading: false });
    useNavigationStore.setState({
      activeProject: { path: PROJECT_PATH, name: 'proj' } as never,
    });
  });

  it('loads the active project memories on mount and lists them', async () => {
    const projectEntries = [makeEntry('k1', 'root cause x')];
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_knowledge_for_project') return Promise.resolve(projectEntries);
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<MemorySection />);

    expect(await screen.findByText('root cause x')).toBeInTheDocument();
    // Scope hint names the project + count.
    expect(screen.getByText(/项目「proj」记忆 1 条/)).toBeInTheDocument();
  });

  it('switches to global search results when typing a query', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_knowledge_for_project')
        return Promise.resolve([makeEntry('k1', 'project only')]);
      if (cmd === 'search_knowledge')
        return Promise.resolve([makeEntry('s1', 'global hit')]);
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<MemorySection />);
    await screen.findByText('project only');

    fireEvent.change(screen.getByLabelText('搜索记忆'), { target: { value: 'query' } });

    expect(await screen.findByText('global hit')).toBeInTheDocument();
    expect(screen.queryByText('project only')).not.toBeInTheDocument();
    expect(screen.getByText(/全局结果 1 条/)).toBeInTheDocument();
  });

  it('deletes a memory via delete_knowledge_entry and removes the card', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'get_knowledge_for_project')
        return Promise.resolve([makeEntry('k1', 'stale lesson')]);
      if (cmd === 'delete_knowledge_entry') return Promise.resolve();
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<MemorySection />);
    await screen.findByText('stale lesson');

    fireEvent.click(screen.getByRole('button', { name: /删除记忆 stale lesson/ }));

    await waitFor(() =>
      expect(toastSpies.success).toHaveBeenCalledWith('记忆已删除，下次任务不再注入'),
    );
    expect(mockInvoke).toHaveBeenCalledWith('delete_knowledge_entry', { id: 'k1' });
    // Card removed from view after the store filters it out.
    expect(screen.queryByText('stale lesson')).not.toBeInTheDocument();
  });
});
