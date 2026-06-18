import { useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { UserHook, UserHookEvent } from '../../types';
import { useToast } from '../Toast';

/**
 * 用户生命周期钩子管理 (D2) — surfaces the user_hooks CRUD backend. Each hook
 * is one shell command bound to a lifecycle event:
 *   - 提交时 (user_prompt_submit): the command's stdout (exit 0) is injected as
 *     context before the turn (claude-code additionalContext analog). Use it to
 *     load project conventions, lint config, or any file into the prompt.
 *   - 停止时 (stop): the command runs for its side effect at run end (output
 *     ignored) — notifications, cleanup, logging.
 *
 * Hooks load into the agent at session start (build_react_agent), so a hook
 * created/toggled here takes effect on the NEXT session submit, no restart.
 * v1 is command-type only and non-blocking (exit 2 logs a warning, never gates).
 */

interface FormState {
  name: string;
  event: UserHookEvent;
  command: string;
  shell: boolean;
  timeoutSecs: number;
  enabled: boolean;
}

const EMPTY_FORM: FormState = {
  name: '',
  event: 'user_prompt_submit',
  command: '',
  shell: true,
  timeoutSecs: 30,
  enabled: true,
};

function toForm(h: UserHook): FormState {
  return {
    name: h.name,
    event: h.event,
    command: h.command,
    shell: h.shell,
    timeoutSecs: h.timeoutSecs,
    enabled: h.enabled,
  };
}

const EVENT_LABEL: Record<UserHookEvent, string> = {
  user_prompt_submit: '提交时',
  stop: '停止时',
};

export function HooksSection() {
  const [hooks, setHooks] = useState<UserHook[]>([]);
  const [loading, setLoading] = useState(true);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [formOpen, setFormOpen] = useState(false);
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const { success, error } = useToast();

  const reload = useCallback(async () => {
    try {
      const list = await invoke<UserHook[]>('list_user_hooks');
      setHooks(Array.isArray(list) ? list : []);
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [error]);

  useEffect(() => {
    reload();
  }, [reload]);

  const openCreate = () => {
    setEditingId(null);
    setForm(EMPTY_FORM);
    setFormOpen(true);
  };

  const openEdit = (h: UserHook) => {
    setEditingId(h.id);
    setForm(toForm(h));
    setFormOpen(true);
  };

  const closeForm = () => {
    setFormOpen(false);
    setEditingId(null);
    setForm(EMPTY_FORM);
  };

  const onSave = async () => {
    const name = form.name.trim();
    if (!name) {
      error('钩子名不能为空');
      return;
    }
    if (!form.command.trim()) {
      error('命令不能为空');
      return;
    }
    setSaving(true);
    try {
      if (editingId) {
        await invoke('update_user_hook', {
          id: editingId,
          name,
          event: form.event,
          command: form.command,
          shell: form.shell,
          timeoutSecs: form.timeoutSecs,
          enabled: form.enabled,
        });
        success(`已更新钩子 ${name}`);
      } else {
        await invoke('create_user_hook', {
          name,
          event: form.event,
          command: form.command,
          shell: form.shell,
          timeoutSecs: form.timeoutSecs,
          enabled: form.enabled,
        });
        success(`已创建钩子 ${name}`);
      }
      closeForm();
      await reload();
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const onToggleEnabled = async (h: UserHook) => {
    try {
      await invoke('set_user_hook_enabled', { id: h.id, enabled: !h.enabled });
      await reload();
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    }
  };

  const onDelete = async (h: UserHook) => {
    if (!window.confirm(`确认删除钩子 ${h.name}？此操作不可撤销。`)) return;
    try {
      await invoke('delete_user_hook', { id: h.id });
      success(`已删除钩子 ${h.name}`);
      await reload();
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="settings-section hooks-section">
      <h3 className="settings-section-title">生命周期钩子</h3>
      <p className="settings-section-desc">
        每个钩子绑定一个 shell 命令到一个生命周期事件：
        <strong> 提交时</strong>（命令 stdout 注入为上下文，如 <code>cat .cursorrules</code>）、
        <strong> 停止时</strong>（运行结束触发副作用，如通知/清理）。
        新建/启用的钩子在<strong>下次会话提交</strong>时生效。
      </p>

      {!formOpen && (
        <button className="provider-btn primary" onClick={openCreate} style={{ marginBottom: 16 }}>
          + 新建钩子
        </button>
      )}

      {formOpen && (
        <div className="memory-card" style={{ marginBottom: 16 }}>
          <div className="memory-card-header">
            <span className="memory-card-title">{editingId ? '编辑钩子' : '新建钩子'}</span>
          </div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10, marginTop: 8 }}>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">名称</span>
              <input
                className="provider-input"
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
                placeholder="例如 load-conventions"
                autoFocus
              />
            </label>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">事件</span>
              <select
                className="provider-input"
                value={form.event}
                onChange={(e) => setForm({ ...form, event: e.target.value as UserHookEvent })}
              >
                <option value="user_prompt_submit">提交时（stdout 注入上下文）</option>
                <option value="stop">停止时（副作用）</option>
              </select>
            </label>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">命令（经 shell 执行）</span>
              <textarea
                className="provider-input"
                value={form.command}
                onChange={(e) => setForm({ ...form, command: e.target.value })}
                placeholder={'cat .cursorrules 2>/dev/null || echo 无项目规则'}
                rows={3}
                style={{ fontFamily: 'monospace' }}
              />
            </label>
            <div style={{ display: 'flex', gap: 16, flexWrap: 'wrap' }}>
              <label style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                <input
                  type="checkbox"
                  checked={form.shell}
                  onChange={(e) => setForm({ ...form, shell: e.target.checked })}
                />
                <span className="settings-section-desc">经 shell（sh -c / cmd /C）</span>
              </label>
              <label style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                <span className="settings-section-desc">超时(秒)</span>
                <input
                  className="provider-input"
                  type="number"
                  min={1}
                  value={form.timeoutSecs}
                  onChange={(e) =>
                    setForm({ ...form, timeoutSecs: Math.max(1, Number(e.target.value) || 30) })
                  }
                  style={{ width: 80 }}
                />
              </label>
              <label style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
                <input
                  type="checkbox"
                  checked={form.enabled}
                  onChange={(e) => setForm({ ...form, enabled: e.target.checked })}
                />
                <span className="settings-section-desc">启用</span>
              </label>
            </div>
            <div style={{ display: 'flex', gap: 8 }}>
              <button className="provider-btn primary" onClick={onSave} disabled={saving}>
                {saving ? '保存中…' : '保存'}
              </button>
              <button className="provider-btn" onClick={closeForm} disabled={saving}>
                取消
              </button>
            </div>
          </div>
        </div>
      )}

      <div className="skills-subhead">全部钩子（{hooks.length}）</div>
      {loading && hooks.length === 0 && <p className="settings-section-desc">加载中...</p>}
      {!loading && hooks.length === 0 && (
        <div className="memory-empty">
          <p>暂无钩子</p>
        </div>
      )}
      <div className="memory-list">
        {hooks.map((h) => (
          <div key={h.id} className="memory-card skills-card">
            <div className="memory-card-header">
              <span className="memory-card-title">{h.name}</span>
              <span className="memory-card-category">{EVENT_LABEL[h.event]}</span>
            </div>
            <p className="memory-card-content" style={{ fontFamily: 'monospace', fontSize: 12, whiteSpace: 'pre-wrap' }}>
              {h.command}
            </p>
            <div className="memory-card-meta">
              <button
                className="memory-card-delete"
                onClick={() => onToggleEnabled(h)}
                aria-label={`切换钩子 ${h.name} 启用状态`}
                title={h.enabled ? '点击禁用' : '点击启用'}
              >
                {h.enabled ? '✓ 已启用' : '○ 已禁用'}
              </button>
              <button className="memory-card-delete" onClick={() => openEdit(h)} aria-label={`编辑钩子 ${h.name}`}>
                编辑
              </button>
              <button className="memory-card-delete" onClick={() => onDelete(h)} aria-label={`删除钩子 ${h.name}`}>
                删除
              </button>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
