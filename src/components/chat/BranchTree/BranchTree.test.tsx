import { describe, it, expect, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { BranchTree, computeActiveChain } from './BranchTree';
import type { BranchNode } from '../../../types';

const makeBranch = (id: string, parentId: string | null, prompt: string): BranchNode => ({
  id, parentId, prompt, status: 'completed', startedAt: '2026-06-22T10:00:00Z', agentType: 'claude',
});

const SAMPLE: BranchNode[] = [
  makeBranch('t1', null, 'turn 1'),
  makeBranch('t2', 't1', 'turn 2'),
  makeBranch('t2fork', 't1', 'turn 2 fork'), // t1 的第二个子（分叉）
  makeBranch('t3', 't2', 'turn 3'),
];

describe('BranchTree', () => {
  it('renders empty state when no branches', () => {
    render(<BranchTree branches={[]} activeLeafId={null} onSelect={() => {}} />);
    expect(screen.getByText('暂无分支')).toBeInTheDocument();
  });

  it('renders all nodes', () => {
    render(<BranchTree branches={SAMPLE} activeLeafId="t3" onSelect={() => {}} />);
    expect(screen.getByText(/^turn 1$/)).toBeInTheDocument();
    expect(screen.getByText(/^turn 2$/)).toBeInTheDocument();
    expect(screen.getByText(/^turn 2 fork/)).toBeInTheDocument();
    expect(screen.getByText(/^turn 3$/)).toBeInTheDocument();
  });

  it('shows branch count in header', () => {
    render(<BranchTree branches={SAMPLE} activeLeafId="t3" onSelect={() => {}} />);
    // 4 turns · 2 branches（t3 和 t2fork 是 leaves）
    expect(screen.getByText(/4 turns/)).toBeInTheDocument();
    expect(screen.getByText(/2 branches/)).toBeInTheDocument();
  });

  it('calls onSelect when node clicked', async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(<BranchTree branches={SAMPLE} activeLeafId="t3" onSelect={onSelect} />);
    await user.click(screen.getByText(/^turn 2 fork/));
    expect(onSelect).toHaveBeenCalledWith('t2fork');
  });

  it('marks active leaf with aria-current', () => {
    render(<BranchTree branches={SAMPLE} activeLeafId="t3" onSelect={() => {}} />);
    const leaf = screen.getByText(/^turn 3$/).closest('button');
    expect(leaf).toHaveAttribute('aria-current', 'true');
  });

  it('shows checkpoint marker for checkpointed nodes', () => {
    render(
      <BranchTree
        branches={SAMPLE}
        activeLeafId="t3"
        onSelect={() => {}}
        checkpointIds={new Set(['t2'])}
      />,
    );
    // t2 节点应有 ◆ 标记（title 含 checkpoint）
    const ckpt = screen.getByTitle('此节点有 git checkpoint（shadow snapshot）');
    expect(ckpt).toBeInTheDocument();
  });

  it('uses role=tree', () => {
    render(<BranchTree branches={SAMPLE} activeLeafId="t3" onSelect={() => {}} />);
    expect(screen.getByRole('tree')).toHaveAttribute('aria-label', '会话分支树');
  });
});

describe('computeActiveChain', () => {
  it('returns path from leaf to root', () => {
    const chain = computeActiveChain(SAMPLE, 't3');
    // t3 → t2 → t1
    expect(chain.has('t3')).toBe(true);
    expect(chain.has('t2')).toBe(true);
    expect(chain.has('t1')).toBe(true);
    expect(chain.has('t2fork')).toBe(false);
  });

  it('returns empty set for null leaf', () => {
    const chain = computeActiveChain(SAMPLE, null);
    expect(chain.size).toBe(0);
  });

  it('handles cycle gracefully', () => {
    const cyclic: BranchNode[] = [
      { id: 'a', parentId: 'b', prompt: 'a', status: 'completed', startedAt: '', agentType: 'x' },
      { id: 'b', parentId: 'a', prompt: 'b', status: 'completed', startedAt: '', agentType: 'x' },
    ];
    const chain = computeActiveChain(cyclic, 'a');
    // 不应死循环
    expect(chain.size).toBe(2);
  });
});
