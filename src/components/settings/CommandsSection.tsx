import { useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { SlashCommand } from '../../types';
import { useToast } from '../Toast';

/**
 * 自定义斜杠命令管理 (D2) — surfaces the CRUD backend (list/create/update/delete
 * slash commands) that was already shipped server-side but had no UI, so
 * create/update/delete were unreachable. Built-ins (category='builtin', seeded by
 * init_db) are read-only here exactly as the backend refuses to mutate them; user
 * commands can be created, edited, and deleted.
 *
 * The kernel expands `/name args` at submit time (commands/agents.rs
 * expand_slash_command), so a command authored here is immediately usable in the
 * composer's `/` trigger menu — no restart needed.
 */

const BUILTIN = 'builtin';

interface FormState {
  name: string;
  description: string;
  template: string;
  category: string;
}

const EMPTY_FORM: FormState = { name: '', description: '', template: '', category: 'user' };

function toForm(c: SlashCommand): FormState {
  return {
    name: c.name,
    description: c.description ?? '',
    template: c.template,
    category: c.category ?? 'user',
  };
}

export function CommandsSection() {
  const [commands, setCommands] = useState<SlashCommand[]>([]);
  const [loading, setLoading] = useState(true);
  const [editingId, setEditingId] = useState<string | null>(null); // null + formOpen=false = closed; null + formOpen=true = create
  const [formOpen, setFormOpen] = useState(false);
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const { success, error } = useToast();

  const reload = useCallback(async () => {
    try {
      const list = await invoke<SlashCommand[]>('list_slash_commands');
      setCommands(Array.isArray(list) ? list : []);
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

  const openEdit = (c: SlashCommand) => {
    setEditingId(c.id);
    setForm(toForm(c));
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
      error('命令名不能为空');
      return;
    }
    if (!form.template.trim()) {
      error('模板不能为空');
      return;
    }
    // Strip a leading slash if the user typed one — names are stored WITHOUT it
    // (parse_command strips it at submit, and the / menu prepends it for display).
    const cleanName = name.replace(/^\/+/, '');
    const description = form.description.trim() || null;
    const category = form.category.trim() || null;
    setSaving(true);
    try {
      if (editingId) {
        await invoke('update_slash_command', {
          id: editingId,
          name: cleanName,
          description,
          template: form.template,
          category,
        });
        success(`已更新命令 /${cleanName}`);
      } else {
        await invoke('create_slash_command', {
          name: cleanName,
          description,
          template: form.template,
          category,
        });
        success(`已创建命令 /${cleanName}`);
      }
      closeForm();
      await reload();
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const onDelete = async (c: SlashCommand) => {
    if (!window.confirm(`确认删除命令 /${c.name}？此操作不可撤销。`)) return;
    try {
      await invoke('delete_slash_command', { id: c.id });
      success(`已删除命令 /${c.name}`);
      await reload();
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="settings-section commands-section">
      <h3 className="settings-section-title">斜杠命令</h3>
      <p className="settings-section-desc">
        内核在提交时把 <code>/name args</code> 展开为模板（<code>$ARGUMENTS</code>/<code>$0</code>
        =全部参数，<code>$1</code>..<code>$n</code>=空格分割的逐个参数）。内置命令只读，用户命令可创建/编辑/删除。
      </p>

      {!formOpen && (
        <button className="provider-btn primary" onClick={openCreate} style={{ marginBottom: 16 }}>
          + 新建命令
        </button>
      )}

      {formOpen && (
        <div className="memory-card" style={{ marginBottom: 16 }}>
          <div className="memory-card-header">
            <span className="memory-card-title">{editingId ? '编辑命令' : '新建命令'}</span>
          </div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10, marginTop: 8 }}>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">命令名（不含 / ）</span>
              <input
                className="provider-input"
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
                placeholder="例如 myreview"
                autoFocus
              />
            </label>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">描述</span>
              <input
                className="provider-input"
                value={form.description}
                onChange={(e) => setForm({ ...form, description: e.target.value })}
                placeholder="命令用途简介"
              />
            </label>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">模板</span>
              <textarea
                className="provider-input"
                value={form.template}
                onChange={(e) => setForm({ ...form, template: e.target.value })}
                placeholder={'审查以下代码并指出问题：$ARGUMENTS'}
                rows={4}
                style={{ fontFamily: 'monospace' }}
              />
            </label>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">分类（留空=默认 user）</span>
              <input
                className="provider-input"
                value={form.category}
                onChange={(e) => setForm({ ...form, category: e.target.value })}
                placeholder="user"
              />
            </label>
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

      <div className="skills-subhead">全部命令（{commands.length}）</div>
      {loading && commands.length === 0 && <p className="settings-section-desc">加载中...</p>}
      {!loading && commands.length === 0 && (
        <div className="memory-empty">
          <p>暂无命令</p>
        </div>
      )}
      <div className="memory-list">
        {commands.map((c) => {
          const isBuiltin = c.category === BUILTIN;
          return (
            <div key={c.id} className="memory-card skills-card">
              <div className="memory-card-header">
                <span className="memory-card-title">/{c.name}</span>
                {c.category && (
                  <span className={`memory-card-category ${isBuiltin ? 'cat-builtin' : ''}`}>
                    {isBuiltin ? '内置' : c.category}
                  </span>
                )}
              </div>
              {c.description && <p className="memory-card-content">{c.description}</p>}
              <p className="memory-card-content" style={{ fontFamily: 'monospace', fontSize: 12, whiteSpace: 'pre-wrap' }}>
                {c.template}
              </p>
              <div className="memory-card-meta">
                {!isBuiltin && (
                  <>
                    <button className="memory-card-delete" onClick={() => openEdit(c)} aria-label={`编辑命令 ${c.name}`}>
                      编辑
                    </button>
                    <button className="memory-card-delete" onClick={() => onDelete(c)} aria-label={`删除命令 ${c.name}`}>
                      删除
                    </button>
                  </>
                )}
                {isBuiltin && <span style={{ color: 'var(--text-muted)', fontSize: 12 }}>内置命令不可编辑</span>}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
