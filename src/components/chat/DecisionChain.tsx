import { useState, useMemo } from 'react';
import type { Session, Requirement } from '../../types';
import { IconEdit, IconCpu, IconTerminal, IconStar, IconX, IconStop } from '../Icons';

interface DecisionChainProps {
  requirement: Requirement | null;
  session: Session | null;
  running: boolean;
}

interface ChainStep {
  icon: React.ReactNode;
  label: string;
  detail: string;
  status: 'done' | 'active' | 'pending';
}

const ICON_SIZE = 14;

export function DecisionChain({ requirement, session, running }: DecisionChainProps) {
  const [collapsed, setCollapsed] = useState(false);

  const steps = useMemo<ChainStep[]>(() => {
    const result: ChainStep[] = [];

    if (requirement) {
      result.push({
        icon: <IconEdit size={ICON_SIZE} />,
        label: '需求定义',
        detail: requirement.title,
        status: 'done',
      });
    }

    if (session) {
      result.push({
        icon: <IconCpu size={ICON_SIZE} />,
        label: 'Agent 执行',
        detail: `${session.agentType}${session.model ? ` · ${session.model}` : ''}`,
        status: running ? 'active' : 'done',
      });

      result.push({
        icon: <IconTerminal size={ICON_SIZE} />,
        label: '指令',
        detail: session.prompt.slice(0, 80) + (session.prompt.length > 80 ? '...' : ''),
        status: running ? 'active' : 'done',
      });
    }

    if (session && !running) {
      const status = session.status === 'completed' ? '完成' :
                     session.status === 'failed' ? '失败' : session.status;
      result.push({
        icon: session.status === 'completed'
          ? <IconStar size={ICON_SIZE} filled />
          : session.status === 'failed'
          ? <IconX size={ICON_SIZE} />
          : <IconStop size={ICON_SIZE} />,
        label: '结果',
        detail: status,
        status: 'done',
      });
    }

    return result;
  }, [requirement, session, running]);

  if (steps.length === 0) return null;

  return (
    <div className="agent-block">
      <div className="agent-block-header" onClick={() => setCollapsed(!collapsed)}>
        <span className="agent-block-title">Decision Chain</span>
        <span className="agent-block-collapse">{collapsed ? '▸' : '▾'}</span>
      </div>
      {!collapsed && (
        <div className="agent-block-body">
          <div className="decision-chain-steps">
            {steps.map((step, i) => (
              <div key={i} className={`decision-chain-step ${step.status}`}>
                <span className="decision-chain-step-icon">{step.icon}</span>
                <div className="decision-chain-step-content">
                  <span className="decision-chain-step-label">{step.label}</span>
                  <span className="decision-chain-step-detail">{step.detail}</span>
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
