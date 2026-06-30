import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, fireEvent } from '@testing-library/react';

// OnboardingWizard pulls Tauri invoke (detect_tools / save_settings) + the
// native dialog plugin (directory picker). Mock both so jsdom never hits the
// native bridge. invoke defaults to [] (detect_tools → empty list) so the
// backend step renders without chaining real IPC.
const invokeMock = vi.fn();
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

import { OnboardingWizard, OnboardingRelaunchSection } from '../OnboardingWizard';
import { useNavigationStore } from '../../../stores/navigationStore';

describe('OnboardingWizard', () => {
  beforeEach(() => {
    invokeMock.mockReset();
    // detect_tools resolves to an empty list — the CLI card shows "未检测到"-less
    // empty state instead of hanging on "检测中…".
    invokeMock.mockResolvedValue([]);
  });

  it('renders as an aria-modal dialog', () => {
    render(<OnboardingWizard onDone={vi.fn()} onClose={vi.fn()} closable={false} />);
    const dialog = document.querySelector('[role="dialog"]');
    expect(dialog).not.toBeNull();
    expect(dialog?.getAttribute('aria-modal')).toBe('true');
  });

  it('hides the close button on first run (closable=false)', () => {
    // First-run wizard must NOT be dismissable — the user has to finish it, so
    // no X button renders. This is the contract App.tsx relies on (closable =
    // onboardingCompleted).
    render(<OnboardingWizard onDone={vi.fn()} onClose={vi.fn()} closable={false} />);
    expect(document.querySelector('.onboarding-close')).toBeNull();
  });

  it('shows the close button on relaunch (closable=true) and fires onClose', () => {
    const onClose = vi.fn();
    render(<OnboardingWizard onDone={vi.fn()} onClose={onClose} closable={true} />);
    const closeBtn = document.querySelector('.onboarding-close') as HTMLElement;
    expect(closeBtn).not.toBeNull();
    fireEvent.click(closeBtn);
    expect(onClose).toHaveBeenCalledTimes(1);
  });

  it('advances through all 4 steps and fires onDone only at the end', () => {
    const onDone = vi.fn();
    const { container } = render(
      <OnboardingWizard onDone={onDone} onClose={vi.fn()} closable={false} />,
    );
    // The footer always has exactly one primary button: "下一步" on steps 0-2,
    // "开始使用" on step 3.
    const clickPrimary = () =>
      fireEvent.click(container.querySelector('button.onboarding-btn.primary') as HTMLElement);

    clickPrimary(); // 0 → 1
    expect(onDone).not.toHaveBeenCalled();
    clickPrimary(); // 1 → 2
    expect(onDone).not.toHaveBeenCalled();
    clickPrimary(); // 2 → 3
    expect(onDone).not.toHaveBeenCalled();
    clickPrimary(); // step 3 → onDone
    expect(onDone).toHaveBeenCalledTimes(1);
  });

  it('disables 上一步 on the first step', () => {
    const { container } = render(
      <OnboardingWizard onDone={vi.fn()} onClose={vi.fn()} closable={false} />,
    );
    const back = container.querySelector('button.onboarding-btn.ghost') as HTMLButtonElement;
    expect(back.disabled).toBe(true);
  });

  it('renders all 4 step labels in the stepper', () => {
    const { container } = render(
      <OnboardingWizard onDone={vi.fn()} onClose={vi.fn()} closable={false} />,
    );
    const labels = Array.from(container.querySelectorAll('.onboarding-step-label')).map((n) =>
      n.textContent?.trim(),
    );
    expect(labels).toEqual(['工作区', '接入方式', '权限说明', '开始使用']);
  });
});

describe('OnboardingRelaunchSection', () => {
  it('renders the relaunch button', () => {
    const { container } = render(<OnboardingRelaunchSection />);
    const btn = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('重新运行新手引导'),
    );
    expect(btn).toBeTruthy();
  });

  it('clicking relaunch opens the wizard overlay (flips onboardingOpen)', () => {
    // The relaunch button is the ONLY entry point that re-opens an
    // already-completed wizard. Clicking must set onboardingOpen=true in the
    // navigation store (App.tsx reads it to mount <OnboardingWizard>).
    useNavigationStore.getState().setOnboardingOpen(false); // reset between tests
    const { container } = render(<OnboardingRelaunchSection />);
    const btn = Array.from(container.querySelectorAll('button')).find((b) =>
      b.textContent?.includes('重新运行新手引导'),
    ) as HTMLElement;
    fireEvent.click(btn);
    // useNavigationStore is a zustand singleton — same instance the component
    // mutated, so reading getState() here observes the click's effect.
    expect(useNavigationStore.getState().onboardingOpen).toBe(true);
  });
});
