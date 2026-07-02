import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { ForgeExperienceSection } from '../ForgeExperienceSection';
import { useNavigationStore } from '../../../stores/navigationStore';
import type { ForgeExperienceReview, Project } from '../../../types';

/**
 * ForgeExperienceSection 补的是 B5 断点：后端 list_pending_forge_reviews /
 * replay_forge_experience 已就绪但前端无入口。覆盖：无项目提示、加载待回顾列表、
 * forge 未装容错、手动回放往返 + 结果展示、空列表时按钮禁用。
 */
const mockInvoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));

function makeReview(taskRef: string, score = 60): ForgeExperienceReview {
  return {
    taskRef,
    score,
    grade: score >= 70 ? 'B' : 'C',
    lowDimensions: [
      { dimension: '测试伴随', score: 40, detail: '改了源码没加测试' },
    ],
    mandatory: true,
    status: 'pending',
    createdAt: '2026-06-29T00:00:00Z',
  };
}

const PROJECT: Project = {
  id: 'p1',
  name: 'demo',
  description: '',
  path: '/proj/demo',
  tags: [],
  cover_image: null,
  open_count: 0,
  last_opened_at: null,
  starred: false,
  created_at: '2026-06-29T00:00:00Z',
  last_opened_tools: [],
  workspace_tools: [],
};

describe('ForgeExperienceSection', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    useNavigationStore.setState({ activeProject: null });
  });

  it('无项目时提示选择项目，且不发起 invoke', () => {
    render(<ForgeExperienceSection />);
    expect(screen.getByText('未选择项目')).toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalled();
  });

  it('有项目时加载待回顾列表并渲染 taskRef 与低维度', async () => {
    useNavigationStore.setState({ activeProject: PROJECT });
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'list_pending_forge_reviews') return Promise.resolve([makeReview('fix/abc')]);
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<ForgeExperienceSection />);

    expect(await screen.findByText('fix/abc')).toBeInTheDocument();
    expect(screen.getByText(/测试伴随/)).toBeInTheDocument();
    expect(screen.getByText(/待回顾任务（1）/)).toBeInTheDocument();
  });

  it('list_pending_forge_reviews 失败（forge 未装）时显示友好提示而非崩溃', async () => {
    useNavigationStore.setState({ activeProject: PROJECT });
    mockInvoke.mockRejectedValue(new Error('ForgeNotInstalled'));
    render(<ForgeExperienceSection />);

    await waitFor(() =>
      expect(screen.getByText(/请确认已安装 Forge CLI/)).toBeInTheDocument(),
    );
  });

  it('点击"重放到知识库"触发 replay_forge_experience 并展示结果', async () => {
    useNavigationStore.setState({ activeProject: PROJECT });
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === 'list_pending_forge_reviews') {
        return Promise.resolve([makeReview('fix/abc')]);
      }
      if (cmd === 'replay_forge_experience') {
        expect(args).toEqual({ projectPath: PROJECT.path });
        return Promise.resolve({ replayed: 1, skipped: 0, promotedGlobal: 1 });
      }
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<ForgeExperienceSection />);
    await screen.findByText('fix/abc');

    fireEvent.click(screen.getByRole('button', { name: '↻ 重放到知识库' }));

    await waitFor(() =>
      expect(screen.getByText(/已回放 1 条，跳过 0 条，提升 1 条跨项目通用经验/)).toBeInTheDocument(),
    );
    expect(mockInvoke).toHaveBeenCalledWith('replay_forge_experience', { projectPath: PROJECT.path });
  });

  it('空列表时回放按钮禁用', async () => {
    useNavigationStore.setState({ activeProject: PROJECT });
    mockInvoke.mockResolvedValue([]);
    render(<ForgeExperienceSection />);

    const btn = await screen.findByRole('button', { name: '↻ 重放到知识库' });
    expect(btn).toBeDisabled();
    expect(screen.getByText(/暂无待回顾经验/)).toBeInTheDocument();
  });
});
