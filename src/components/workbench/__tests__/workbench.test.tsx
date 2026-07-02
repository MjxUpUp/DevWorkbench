import { describe, it, expect, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { PlanBar } from '../PlanBar';
import { GateBar } from '../GateBar';
import { useNavigationStore } from '../../../stores/navigationStore';
import { useAgentStore } from '../../../stores/agentStore';
import type { Session, SessionStatus } from '../../../types';

/**
 * 起底重构 块1 骨架契约测试。
 *
 * 4 区结构与 Stage 视图路由的正确性由 E2E 覆盖（app.spec → chat 路径，
 * orchestrate.spec → orchestrate 路径，均 7/7 绿）。此处只单测两条纯派生逻辑：
 *  - PlanBar：activeView → 运行模式（轴线A「谁持有 plan」的 UI 锚点）
 *  - GateBar：sessions → 运行计数（门控层实时态）
 * 派生函数越纯越该单测；带副作用/渲染的集成留给 E2E。
 */

// 最小合法 Session——GateBar 只读 status，其余字段对逻辑无关，但仍按接口填全
// 避免未来字段收紧时静默漏配。
const mkSession = (status: SessionStatus): Session => ({
  id: `s-${status}`,
  projectPath: '/p',
  agentType: 'react_kernel',
  status,
  prompt: '',
  model: null,
  startedAt: '2026-01-01T00:00:00Z',
  finishedAt: null,
  exitCode: null,
  outputSummary: null,
  contextSnapshot: null,
  linkedRequirementId: null,
  parentSessionId: null,
  conversationId: null,
});

describe('PlanBar — 视图→运行模式派生（轴线A）', () => {
  beforeEach(() => {
    useNavigationStore.setState({ activeView: 'task', activeProject: null });
  });

  it('task 视图 → Chat Agent，plan∈LLM context', () => {
    useNavigationStore.setState({ activeView: 'task' });
    render(<PlanBar />);
    expect(screen.getByTestId('plan-mode')).toHaveTextContent('Chat Agent');
    expect(screen.getByTestId('plan-bar')).toHaveTextContent(/plan ∈ LLM context/);
  });

  it('orchestrate 视图 → DAG Script，plan∈脚本变量', () => {
    useNavigationStore.setState({ activeView: 'orchestrate' });
    render(<PlanBar />);
    expect(screen.getByTestId('plan-mode')).toHaveTextContent('DAG Script');
    expect(screen.getByTestId('plan-bar')).toHaveTextContent(/plan ∈ 脚本变量/);
  });

  it('trace 视图 → 观测模式', () => {
    useNavigationStore.setState({ activeView: 'trace' });
    render(<PlanBar />);
    expect(screen.getByTestId('plan-mode')).toHaveTextContent('观测');
  });
});

describe('GateBar — 运行态派生（门控层）', () => {
  beforeEach(() => {
    useAgentStore.setState({ sessions: [] });
  });

  it('无 running 会话 → idle', () => {
    useAgentStore.setState({
      sessions: [mkSession('completed'), mkSession('failed')],
    });
    render(<GateBar />);
    expect(screen.getByTestId('gate-bar')).toHaveTextContent(/idle/);
    expect(screen.getByTestId('gate-bar').querySelector('[data-running="false"]')).not.toBeNull();
  });

  it('有 running 会话 → 计数显示且 data-running=true', () => {
    useAgentStore.setState({
      sessions: [
        mkSession('running'),
        mkSession('running'),
        mkSession('completed'),
      ],
    });
    render(<GateBar />);
    expect(screen.getByTestId('gate-bar')).toHaveTextContent(/2 个 agent 运行中/);
    expect(screen.getByTestId('gate-bar').querySelector('[data-running="true"]')).not.toBeNull();
  });
});
