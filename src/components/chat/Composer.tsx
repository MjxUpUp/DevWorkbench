import React, { useState, useRef } from 'react';
import { IconPlay, IconStop } from '../Icons';
import { TriggerMenu } from '../TriggerMenu';
import { Button } from '../ui/Button/Button';

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
  /** Steering 模式（Cursor 3.0 / Codex app 范式）：
   * 运行中时允许输入插话/排队消息。true 时显示双行提示 + 不禁用 textarea。
   * Enter=插话（当前工具后送达）/ Shift+Enter=排队（完成后送达）。 */
  steering?: boolean;
  /** 发送 steering 消息（插话）。只 steering=true 时可用。 */
  onSteer?: () => void;
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
  steering = false,
  onSteer,
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
    // Steering 模式：运行中时 Enter 插话（不走默认 Ctrl+Enter 发送）
    if (steering && isRunning && e.key === 'Enter' && !e.shiftKey && prompt.trim() && onSteer) {
      e.preventDefault();
      onSteer();
      return;
    }
    // 普通 Enter 发送（非 steering + 非运行 + Shift+Enter 换行）
    if (e.key === 'Enter' && !e.shiftKey && canSend && !isRunning) {
      e.preventDefault();
      onSend();
      return;
    }
    // Ctrl+Enter 发送（兼容旧习惯，同时用于 steering 已占用 Enter 的场景）
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

  // Explicit trigger buttons (@ file / / command / $ skill) — mirrors zcode's
  // composer which surfaces these as visible glyphs rather than relying on the
  // user typing the trigger character. These live in a compact toolbar above
  // the input so the bottom action bar stays uncluttered.
  const openTrigger = (type: '@' | '/' | '$') => {
    setTriggerMenu((prev) => (prev?.type === type ? null : { type, position: { top: 0, left: 0 } }));
  };

  return (
    <div className="chat-composer">
      {/* Steering 提示条（运行中 + steering 开启时显示）*/}
      {isRunning && steering && (
        <div className="composer-steering-hint" role="status">
          <span className="composer-steering-icon" aria-hidden="true">⚠</span>
          <span className="composer-steering-label">STEERING MODE · agent 运行中</span>
          <span className="composer-steering-desc">⏎ Enter = 插话（当前工具后送达）· ⇧⏎ Shift+Enter = 排队（完成后送达）</span>
        </div>
      )}

      {attachedFiles.length > 0 && (
        <div className="file-chips">
          {attachedFiles.map((file) => (
            <span key={file.path} className="file-chip">
              @{file.name}
              <Button variant="dangerGhost" size="sm" onClick={() => onRemoveFile(file.path)} aria-label="移除文件">×</Button>
            </span>
          ))}
        </div>
      )}

      <div className="chat-composer-input-wrap">
        {triggerMenu && (
          <div className="composer-trigger-popover">
            <TriggerMenu
              type={triggerMenu.type}
              position={triggerMenu.position}
              onSelect={handleTriggerSelect}
              onClose={() => setTriggerMenu(null)}
            />
          </div>
        )}

        {/* Compact trigger toolbar (kept above the input so the bottom row stays clean) */}
        <div className="composer-triggers">
          <button
            type="button"
            className="composer-trigger-btn"
            data-active={triggerMenu?.type === '@' || undefined}
            onClick={() => openTrigger('@')}
            title="附加文件 (@)"
          >@</button>
          <button
            type="button"
            className="composer-trigger-btn"
            data-active={triggerMenu?.type === '/' || undefined}
            onClick={() => openTrigger('/')}
            title="命令 (/)"
          >/</button>
          <button
            type="button"
            className="composer-trigger-btn"
            data-active={triggerMenu?.type === '$' || undefined}
            onClick={() => openTrigger('$')}
            title="技能 ($)"
          >$</button>
        </div>

        <textarea
          ref={textareaRef}
          className="chat-composer-input"
          data-testid="chat-composer-input"
          placeholder={isRunning && steering ? '插话/排队消息（Enter 插话 · Shift+Enter 排队）...' : (placeholder ?? '提出后续修改要求...')}
          value={prompt}
          onChange={handlePromptChange}
          onKeyDown={handleKeyDown}
          disabled={isRunning && !steering}
          maxLength={10000}
          rows={1}
        />
      </div>

      {/* Bottom action bar: 发送 / 停止（模式选择器已移除，破坏性操作走 ApprovalModal）*/}
      <div className="composer-actions">
        <div className="composer-actions-left"></div>
        {isRunning ? (
          <button className="composer-send-btn stop" onClick={onStop} title="停止" data-testid="composer-send-btn">
            <IconStop size={16} />
          </button>
        ) : (
          <button
            className="composer-send-btn send"
            onClick={onSend}
            disabled={!canSend}
            title="发送 (Ctrl+Enter)"
            data-testid="composer-send-btn"
          >
            <IconPlay size={16} />
          </button>
        )}
      </div>
    </div>
  );
}
