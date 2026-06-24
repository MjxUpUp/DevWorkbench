import { useMemo, type HTMLAttributes } from 'react';
import type { BranchNode } from '../../../types';
import styles from './BranchTree.module.css';

/**
 * BranchTree — conversation 分支树视图。
 *
 * 把 ChatView 内联的分支切换器升级为独立的可视化树：
 * - 纵向缩进表示 parent→child 层级
 * - 树连接符（├─ └─ │）还原分支结构
 * - 当前分支链高亮（accent 色），leaf 带 CRT 闪烁
 * - fork 节点（edit&regenerate 产生）用陶土色标记
 * - checkpoint 节点用 sunkissed 色 ◆ 标记（hover 显示信息）
 *
 * 数据来自后端 get_conversation_branches 返回的 BranchNode[]（扁平 + parentId 指针）。
 *
 * a11y：节点用 button + aria-current，树用 role="tree"。
 */
export interface BranchTreeProps {
  /** 扁平分支节点列表（含 parentId 指针）。 */
  branches: BranchNode[];
  /** 当前激活的 leaf 节点 id（当前查看的分支末端）。 */
  activeLeafId: string | null;
  /** 当前分支链上所有节点 id 的集合（从 root 到 activeLeaf 的路径）。 */
  activeChainIds?: Set<string>;
  /** 节点点击回调（切换到该分支）。 */
  onSelect: (nodeId: string) => void;
  /** 哪些节点有 checkpoint（id 集合，可选，用于显示 ◆ 标记）。 */
  checkpointIds?: Set<string>;
  className?: string;
}

export function BranchTree({
  branches,
  activeLeafId,
  activeChainIds,
  onSelect,
  checkpointIds,
  className,
  ...props
}: BranchTreeProps & HTMLAttributes<HTMLDivElement>) {
  // parentId → children 分组
  const childrenByParent = useMemo(() => {
    const m = new Map<string | null, BranchNode[]>();
    for (const b of branches) {
      const arr = m.get(b.parentId) ?? [];
      arr.push(b);
      m.set(b.parentId, arr);
    }
    return m;
  }, [branches]);

  // 找根节点（parentId 为 null）
  const roots = childrenByParent.get(null) ?? [];

  // 递归渲染节点
  const renderNode = (node: BranchNode, depth: number, isLast: boolean, prefix: string): React.ReactNode => {
    const children = childrenByParent.get(node.id) ?? [];
    const isActiveChain = activeChainIds?.has(node.id) ?? false;
    const isLeaf = node.id === activeLeafId;
    const isFork = children.length > 1; // 多子节点 = 分叉点
    const hasCkpt = checkpointIds?.has(node.id) ?? false;

    const nodeClasses = [
      styles.node,
      isActiveChain ? styles.activeChain : '',
      isLeaf ? styles.leaf : '',
      isFork ? styles.fork : '',
    ].filter(Boolean).join(' ');

    return (
      <div key={node.id}>
        <button
          type="button"
          className={nodeClasses}
          onClick={() => onSelect(node.id)}
          aria-current={isLeaf ? 'true' : undefined}
          title={node.prompt}
        >
          {/* 树连接符 */}
          {depth > 0 && (
            <span className={styles.connector} aria-hidden="true">{prefix}{isLast ? '└─ ' : '├─ '}</span>
          )}
          <span className={styles.dot} aria-hidden="true" />
          <span className={styles.label}>{truncate(node.prompt, 50)}</span>
          {hasCkpt && (
            <span className={styles.ckpt} title="此节点有 git checkpoint（shadow snapshot）">◆</span>
          )}
          <span className={styles.meta}>{node.agentType}</span>
        </button>
        {/* 递归子节点 */}
        {children.length > 0 && (
          <div>
            {children.map((child, i) => {
              const childIsLast = i === children.length - 1;
              const childPrefix = depth === 0 ? '' : prefix + (isLast ? '   ' : '│  ');
              return renderNode(child, depth + 1, childIsLast, childPrefix);
            })}
          </div>
        )}
      </div>
    );
  };

  const wrapClasses = [styles.wrap, className].filter(Boolean).join(' ');

  if (branches.length === 0) {
    return (
      <div className={wrapClasses} {...props}>
        <div className={styles.empty}>暂无分支</div>
      </div>
    );
  }

  return (
    <div className={wrapClasses} role="tree" aria-label="会话分支树" {...props}>
      <div className={styles.head}>
        <span className={styles.headTitle}>BRANCH TREE</span>
        <span className={styles.headMeta}>{branches.length} turns · {countLeaves(branches)} branches</span>
      </div>
      <div className={styles.nodes}>
        {roots.map((root, i) => {
          const isLast = i === roots.length - 1;
          return renderNode(root, 0, isLast, '');
        })}
      </div>
    </div>
  );
}

/** 从 activeLeaf 往上走到 root，收集路径上所有 id。 */
export function computeActiveChain(branches: BranchNode[], activeLeafId: string | null): Set<string> {
  if (!activeLeafId) return new Set();
  const byId = new Map(branches.map((b) => [b.id, b]));
  const chain = new Set<string>();
  let cursor: string | null = activeLeafId;
  const visited = new Set<string>();
  while (cursor && !visited.has(cursor)) {
    visited.add(cursor);
    chain.add(cursor);
    const node = byId.get(cursor);
    cursor = node?.parentId ?? null;
  }
  return chain;
}

function truncate(s: string, n: number): string {
  return s.length <= n ? s : s.slice(0, n) + '...';
}

function countLeaves(branches: BranchNode[]): number {
  const parentIds = new Set(branches.map((b) => b.parentId));
  // leaves = 没有任何节点把它当 parent 的节点
  return branches.filter((b) => !parentIds.has(b.id)).length;
}
