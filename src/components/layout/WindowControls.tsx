import { getCurrentWindow } from '@tauri-apps/api/window';
import { isTauri } from '../../utils/env';

/**
 * Custom window controls for the frameless title bar (zcode/VSCode style):
 * minimize / maximize-restore / close.
 *
 * Pure presentational + action component — the maximized state is owned by the
 * parent (TitleBar), which both toggles the icon here and applies the
 * `.window-maximized` padding to the app root.
 *
 * In a plain browser (no Tauri IPC) the buttons render for layout verification
 * but invoke no window API — `isTauri()` short-circuits each call.
 */
export function WindowControls({ maximized }: { maximized: boolean }) {
  const tauri = isTauri();

  const minimize = () => { if (tauri) getCurrentWindow().minimize().catch(() => {}); };
  const toggleMaximize = () => { if (tauri) getCurrentWindow().toggleMaximize().catch(() => {}); };
  const close = () => { if (tauri) getCurrentWindow().close().catch(() => {}); };

  return (
    <div className="window-controls" role="group" aria-label="窗口控制">
      <button className="window-control" onClick={minimize} title="最小化" aria-label="最小化" type="button">
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <line x1="1" y1="5" x2="9" y2="5" stroke="currentColor" strokeWidth="1" />
        </svg>
      </button>
      <button
        className="window-control"
        onClick={toggleMaximize}
        title={maximized ? '还原' : '最大化'}
        aria-label={maximized ? '还原' : '最大化'}
        type="button"
      >
        {maximized ? (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
            <rect x="1" y="3.5" width="5.5" height="5.5" stroke="currentColor" strokeWidth="1" />
            <rect x="3.5" y="1" width="5.5" height="5.5" stroke="currentColor" strokeWidth="1" />
          </svg>
        ) : (
          <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
            <rect x="1.5" y="1.5" width="7" height="7" stroke="currentColor" strokeWidth="1" />
          </svg>
        )}
      </button>
      <button className="window-control window-control-close" onClick={close} title="关闭" aria-label="关闭" type="button">
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" strokeWidth="1" />
          <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" strokeWidth="1" />
        </svg>
      </button>
    </div>
  );
}
