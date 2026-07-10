import { useState } from 'react';
import { useSettingsStore } from '../../stores/settingsStore';
import { useNavigationStore } from '../../stores/navigationStore';
import {
  IconX,
  IconFolderOpen,
  IconCpu,
  IconShield,
  IconChevronRight,
  IconSparkles,
} from '../Icons';

/**
 * First-run onboarding overlay. Auto-shows when `settings.onboarding_completed`
 * is false (fresh install) OR when the user re-launches it from Settings → 引导.
 *
 * Design intent: a lightweight guide, not a second settings page. Each step
 * either explains ONE concept or wires ONE thing (workspace root / agent backend
 * awareness); deep configuration stays in Settings. `onDone` flips
 * onboarding_completed → true so the overlay never auto-shows again.
 *
 * Why overlay-at-App-root (not inside SettingsView): the first-run case has no
 * Settings view mounted yet — the wizard must render from App regardless of
 * activeView, sitting above every other overlay.
 */
export interface OnboardingWizardProps {
  /** Flipped when the user finishes the wizard → caller saves onboarding_completed. */
  onDone: () => void;
  /** Closes the overlay WITHOUT flipping the completed flag (relaunch path only). */
  onClose: () => void;
  /** First run = false (no X button, must finish); relaunch = true (X allowed). */
  closable: boolean;
}

type StepId = 0 | 1 | 2 | 3;

const STEP_LABELS = ['工作区', '接入方式', '权限说明', '开始使用'] as const;

export function OnboardingWizard({ onDone, onClose, closable }: OnboardingWizardProps) {
  const [step, setStep] = useState<StepId>(0);
  const [workspacePath, setWorkspacePath] = useState<string>('');
  const [pickError, setPickError] = useState<string | null>(null);

  const scanDirectories = useSettingsStore((s) => s.settings.scan_directories);
  const saveSettings = useSettingsStore((s) => s.saveSettings);

  const pickWorkspace = async () => {
    setPickError(null);
    try {
      const { open } = await import('@tauri-apps/plugin-dialog');
      const selected = await open({ directory: true, multiple: false });
      if (typeof selected === 'string' && selected.length > 0) {
        setWorkspacePath(selected);
        // Dedupe so re-picking the same root doesn't grow the list.
        const next = Array.from(new Set([...scanDirectories, selected]));
        await saveSettings({ scan_directories: next });
      }
    } catch (e) {
      setPickError('目录选择不可用，可稍后在「设置」中添加扫描目录。');
      console.error('[onboarding] workspace pick failed', e);
    }
  };

  const goNext = () => setStep((s) => (s < 3 ? ((s + 1) as StepId) : s));
  const goBack = () => setStep((s) => (s > 0 ? ((s - 1) as StepId) : s));

  return (
    <div className="onboarding-overlay" role="dialog" aria-modal="true" aria-label="新手引导">
      <div className="onboarding-card">
        <div className="onboarding-header">
          <div className="onboarding-stepper">
            {STEP_LABELS.map((label, i) => (
              <div
                key={label}
                className={`onboarding-step-dot ${i === step ? 'active' : ''} ${i < step ? 'done' : ''}`}
              >
                <span className="onboarding-step-num">{i < step ? '✓' : i + 1}</span>
                <span className="onboarding-step-label">{label}</span>
              </div>
            ))}
          </div>
          {closable && (
            <button className="onboarding-close" onClick={onClose} aria-label="关闭引导" type="button">
              <IconX size={16} />
            </button>
          )}
        </div>

        <div className="onboarding-body">
          {step === 0 && (
            <div className="onboarding-step">
              <IconFolderOpen size={40} />
              <h2>授权你的工作区</h2>
              <p>
                选择一个根目录，Agent 将在此范围内读写文件、运行命令。可随时在设置中增改，也可跳过。
              </p>
              <button className="onboarding-action" type="button" onClick={pickWorkspace}>
                <IconFolderOpen size={16} /> 选择工作区目录
              </button>
              {workspacePath && (
                <div className="onboarding-picked">
                  已授权：<code>{workspacePath}</code>
                </div>
              )}
              {pickError && <div className="onboarding-hint">{pickError}</div>}
            </div>
          )}

          {step === 1 && (
            <div className="onboarding-step">
              <h2>选择接入方式</h2>
              <p>接入云模型即可开始——自研内核（ReactKernel）直接调度，无需安装外部 CLI。</p>
              <div className="onboarding-cards">
                <div className="onboarding-card-option">
                  <IconCpu size={28} />
                  <h3>云模型</h3>
                  <p>
                    填入供应商 API Key（Anthropic / GLM / DeepSeek …），自研内核直接调度。API Key
                    仅存本机钥匙串。
                  </p>
                </div>
              </div>
              <div className="onboarding-hint">完成后可在「设置 → 模型供应商」详细配置。</div>
            </div>
          )}

          {step === 2 && (
            <div className="onboarding-step">
              <IconShield size={40} />
              <h2>权限与安全说明</h2>
              <ul className="onboarding-perm-list">
                <li>
                  <b>破坏性操作拦截</b>：删除文件、覆盖已存在文件、强制推送等高危动作会弹出审批，你
                  Approve / Reject / Retry 后才执行。
                </li>
                <li>
                  <b>上下文自动压缩</b>：长会话接近上下文上限时，中间轮次会被折叠为摘要并归档，原文可展开查看。
                </li>
                <li>
                  <b>本地优先</b>：代码、会话、凭据都留在本机；云模型只在你主动调用时联网。
                </li>
              </ul>
            </div>
          )}

          {step === 3 && (
            <div className="onboarding-step onboarding-step-done">
              <IconSparkles size={40} />
              <h2>一切就绪</h2>
              <p>你可以开始工作了。需要时从左侧「设置 → 引导」重新打开本向导。</p>
            </div>
          )}
        </div>

        <div className="onboarding-footer">
          <button className="onboarding-btn ghost" type="button" onClick={goBack} disabled={step === 0}>
            上一步
          </button>
          {step < 3 ? (
            <button className="onboarding-btn primary" type="button" onClick={goNext}>
              下一步 <IconChevronRight size={16} />
            </button>
          ) : (
            <button className="onboarding-btn primary" type="button" onClick={onDone}>
              <IconSparkles size={16} /> 开始使用
            </button>
          )}
        </div>
      </div>
    </div>
  );
}

/**
 * The Settings → 引导 section body. Just a relaunch button that re-opens the
 * wizard overlay: sets onboardingOpen=true and leaves the settings view so the
 * full-screen overlay is visible. Mirrors the addProjectOpen open/close pattern
 * in navigationStore.
 */
export function OnboardingRelaunchSection() {
  const setOnboardingOpen = useNavigationStore((s) => s.setOnboardingOpen);
  const setActiveView = useNavigationStore((s) => s.setActiveView);
  return (
    <div className="onboarding-relaunch">
      <h3>新手引导</h3>
      <p>
        重新走一遍首次启动向导：授权工作区、选择接入方式（云模型或 CLI）、了解权限与安全模型。不会清除你已有的任何配置。
      </p>
      <button
        type="button"
        className="onboarding-btn primary"
        onClick={() => {
          setOnboardingOpen(true);
          setActiveView('task');
        }}
      >
        <IconSparkles size={16} /> 重新运行新手引导
      </button>
    </div>
  );
}
