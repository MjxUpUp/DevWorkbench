import { createContext, useContext, useState, useCallback, useRef } from 'react';
import type { ReactNode } from 'react';

export type ToastType = 'success' | 'error' | 'info';

/** Optional inline action (e.g. an "撤销" undo button on a delete toast). */
export interface ToastAction {
  label: string;
  onClick: () => void;
}

interface Toast {
  id: number;
  message: string;
  type: ToastType;
  action?: ToastAction;
}

interface ToastContextValue {
  toast: (type: ToastType, message: string, action?: ToastAction) => void;
  success: (message: string, action?: ToastAction) => void;
  error: (message: string, action?: ToastAction) => void;
  info: (message: string, action?: ToastAction) => void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

export function ToastProvider({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextId = useRef(0);

  const removeToast = useCallback((id: number) => {
    setToasts(prev => prev.filter(t => t.id !== id));
  }, []);

  const addToast = useCallback((type: ToastType, message: string, action?: ToastAction) => {
    const id = ++nextId.current;
    setToasts(prev => [...prev, { id, message, type, action }]);
    setTimeout(() => removeToast(id), 3000);
  }, [removeToast]);

  const value: ToastContextValue = {
    toast: addToast,
    success: (message, action) => addToast('success', message, action),
    error: (message, action) => addToast('error', message, action),
    info: (message, action) => addToast('info', message, action),
  };

  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toast-container">
        {toasts.map(t => (
          <div key={t.id} className={`toast toast-${t.type}`} onClick={() => removeToast(t.id)}>
            <span className="toast-message">{t.message}</span>
            {t.action && (
              <button
                className="toast-action"
                type="button"
                onClick={(e) => {
                  e.stopPropagation();
                  t.action!.onClick();
                  removeToast(t.id);
                }}
              >
                {t.action.label}
              </button>
            )}
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error('useToast must be used within ToastProvider');
  return ctx;
}
