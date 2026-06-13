import React, { useState, useRef } from 'react';
import { IconPlay, IconStop } from '../Icons';
import { TriggerMenu } from '../TriggerMenu';

interface AttachedFile {
  path: string;
  name: string;
}

interface ComposerProps {
  prompt: string;
  onPromptChange: (value: string) => void;
  onSend: () => void;
  onStop: () => void;
  canSend: boolean;
  isRunning: boolean;
  attachedFiles: AttachedFile[];
  onAttachFile: (file: AttachedFile) => void;
  onRemoveFile: (path: string) => void;
  placeholder?: string;
}

export function Composer({
  prompt,
  onPromptChange,
  onSend,
  onStop,
  canSend,
  isRunning,
  attachedFiles,
  onAttachFile,
  onRemoveFile,
  placeholder,
}: ComposerProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const [triggerMenu, setTriggerMenu] = useState<{ type: '@' | '/' | '$'; position: { top: number; left: number } } | null>(null);

  const handlePromptChange = (e: React.ChangeEvent<HTMLTextAreaElement>) => {
    onPromptChange(e.target.value);
    const el = e.target;
    el.style.height = 'auto';
    el.style.height = Math.min(el.scrollHeight, 180) + 'px';
  };

  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Ctrl+Enter to send
    if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && canSend) {
      e.preventDefault();
      onSend();
      return;
    }

    // Trigger characters
    if (e.key === '@' || e.key === '/' || e.key === '$') {
      const textarea = e.currentTarget;
      const text = textarea.value;
      const cursorPos = textarea.selectionStart;
      const beforeChar = cursorPos === 0 ? ' ' : text[cursorPos - 1];
      if (beforeChar === ' ' || beforeChar === '\n' || cursorPos === 0) {
        setTriggerMenu({ type: e.key as '@' | '/' | '$', position: { top: 0, left: 0 } });
      }
    }

    if (e.key === 'Escape' && triggerMenu) {
      setTriggerMenu(null);
    }
  };

  const handleTriggerSelect = (item: { label: string; path?: string }) => {
    setTriggerMenu(null);
    if (triggerMenu?.type === '@') {
      const file: AttachedFile = { path: item.path || item.label, name: item.label };
      if (!attachedFiles.some((f) => f.path === file.path)) {
        onAttachFile(file);
      }
      if (prompt.endsWith('@')) {
        onPromptChange(prompt.slice(0, -1));
      }
    } else if (triggerMenu?.type === '/') {
      const trimmed = prompt.endsWith('/') ? prompt.slice(0, -1) : prompt;
      onPromptChange(trimmed + item.label + ' ');
    } else if (triggerMenu?.type === '$') {
      const trimmed = prompt.endsWith('$') ? prompt.slice(0, -1) : prompt;
      onPromptChange(trimmed + `[${item.label}] `);
    }
    textareaRef.current?.focus();
  };

  return (
    <div className="chat-composer">
      {triggerMenu && (
        <TriggerMenu
          type={triggerMenu.type}
          position={triggerMenu.position}
          onSelect={handleTriggerSelect}
          onClose={() => setTriggerMenu(null)}
        />
      )}

      {attachedFiles.length > 0 && (
        <div className="file-chips">
          {attachedFiles.map((file) => (
            <span key={file.path} className="file-chip">
              @{file.name}
              <button className="file-chip-remove" onClick={() => onRemoveFile(file.path)}>×</button>
            </span>
          ))}
        </div>
      )}

      <div className="chat-composer-input-wrap">
        <textarea
          ref={textareaRef}
          className="chat-composer-input"
          placeholder={placeholder ?? '输入需求... @ 文件 / 命令 $ 技能'}
          value={prompt}
          onChange={handlePromptChange}
          onKeyDown={handleKeyDown}
          disabled={isRunning}
          maxLength={10000}
          rows={1}
        />
        <button
          className="composer-attach-btn"
          title="附加文件"
          onClick={() => setTriggerMenu({ type: '@', position: { top: 0, left: 0 } })}
        >
          ⊕
        </button>
        {isRunning ? (
          <button className="composer-send-btn stop" onClick={onStop} title="停止">
            <IconStop size={16} />
          </button>
        ) : (
          <button
            className="composer-send-btn send"
            onClick={onSend}
            disabled={!canSend}
            title="发送 (Ctrl+Enter)"
          >
            <IconPlay size={16} />
          </button>
        )}
      </div>
    </div>
  );
}
