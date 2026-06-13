import React, { useState, useRef } from 'react';
import { IconPlay, IconStop } from '../Icons';
import { TriggerMenu } from '../TriggerMenu';
import { ModelSelector } from '../ModelSelector';
import type { AgentMode } from '../ModeSelector';

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
  /** Agent execution mode — surfaces a "计划模式" toggle in the action bar. */
  agentMode?: AgentMode;
  onModeChange?: (mode: AgentMode) => void;
  /** Selected model id — surfaces a model picker in the action bar. */
  selectedModel?: string;
  onModelChange?: (model: string) => void;
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
  agentMode,
  onModeChange,
  selectedModel,
  onModelChange,
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

  // Explicit trigger buttons (@ file / / command / $ skill) — mirrors zcode's
  // composer which surfaces these as visible glyphs rather than relying on the
  // user typing the trigger character. These live in a compact toolbar above
  // the input so the bottom action bar stays uncluttered.
  const openTrigger = (type: '@' | '/' | '$') => {
    setTriggerMenu((prev) => (prev?.type === type ? null : { type, position: { top: 0, left: 0 } }));
  };

  // "计划模式" toggle — flips between the user's current mode and plan mode.
  // When on, it shows as active regardless of which non-plan mode was set; the
  // underlying value switches between 'plan' and 'default'.
  const planActive = agentMode === 'plan';
  const togglePlan = () => {
    if (!onModeChange) return;
    onModeChange(planActive ? 'default' : 'plan');
  };

  return (
    <div className="chat-composer">
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
          placeholder={placeholder ?? '提出后续修改要求...'}
          value={prompt}
          onChange={handlePromptChange}
          onKeyDown={handleKeyDown}
          disabled={isRunning}
          maxLength={10000}
          rows={1}
        />
      </div>

      {/* Bottom action bar: 计划模式 · 模型选择 · 发送 (aligns to target mockup) */}
      <div className="composer-actions">
        <div className="composer-actions-left">
          {onModeChange && (
            <button
              type="button"
              className={`composer-action-btn ${planActive ? 'active' : ''}`}
              onClick={togglePlan}
              title="计划模式 — 先输出计划，确认后执行"
            >
              计划模式
            </button>
          )}
          {onModelChange && (
            <ModelSelector value={selectedModel ?? 'default'} onChange={onModelChange} />
          )}
        </div>
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
