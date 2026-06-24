import { useEffect, useState, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import type { SubAgentInfo, SubAgentScope } from '../../types';
import { useToast } from '../Toast';
import { Button } from '../ui/Button/Button';
import { useNavigationStore } from '../../stores/navigationStore';

/**
 * 命名子智能体编辑 (D1) — file-based CRUD over `.agents/subagents/<name>/
 * AGENT.md`. The kernel's SubAgentTool already loads these files and delegates
 * by name (`dispatch_subagent {subagent: "researcher"}`); the gap was that
 * users had to hand-author the AGENT.md frontmatter. This page writes valid
 * frontmatter (name/description/system_prompt/tools_allow) so a sub-agent
 * created here is immediately delegatable on the next session submit.
 *
 * Scopes: global (~/.agents/subagents, shared) or project (<project>/
 * .agents/subagents, versioned). The agent loads all three tiers (global →
 * project → app-private); earlier scopes shadow same-named later ones, which
 * list_subagents already dedupes to surface.
 */

interface FormState {
  name: string;
  description: string;
  systemPrompt: string;
  /** Entered comma-separated; stored as an array. */
  toolsAllow: string;
  scope: SubAgentScope;
  /** Original name when editing (rename not supported — keeps path stable). */
  originalName: string | null;
}

const EMPTY_FORM: FormState = {
  name: '',
  description: '',
  systemPrompt: '',
  toolsAllow: '',
  scope: 'project',
  originalName: null,
};

function toForm(s: SubAgentInfo): FormState {
  return {
    name: s.name,
    description: s.description,
    systemPrompt: s.systemPrompt,
    toolsAllow: s.toolsAllow.join(', '),
    scope: s.scope === 'global' ? 'global' : 'project',
    originalName: s.name,
  };
}

/** Parse a comma-separated tools-allow string into trimmed, non-empty prefixes. */
function parseToolsAllow(raw: string): string[] {
  return raw
    .split(',')
    .map((t) => t.trim())
    .filter((t) => t.length > 0);
}

const SCOPE_LABEL: Record<string, string> = {
  global: '全局',
  project: '项目',
  'app-private': '应用',
};

export function SubAgentsSection() {
  const [agents, setAgents] = useState<SubAgentInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [formOpen, setFormOpen] = useState(false);
  const [form, setForm] = useState<FormState>(EMPTY_FORM);
  const [saving, setSaving] = useState(false);
  const activeProject = useNavigationStore((s) => s.activeProject);
  const { success, error } = useToast();

  const reload = useCallback(async () => {
    try {
      const list = await invoke<SubAgentInfo[]>('list_subagents', {
        projectPath: activeProject?.path ?? null,
      });
      setAgents(Array.isArray(list) ? list : []);
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [activeProject, error]);

  useEffect(() => {
    reload();
  }, [reload]);

  const openCreate = () => {
    setForm({ ...EMPTY_FORM, scope: activeProject ? 'project' : 'global' });
    setFormOpen(true);
  };

  const openEdit = (s: SubAgentInfo) => {
    setForm(toForm(s));
    setFormOpen(true);
  };

  const closeForm = () => {
    setFormOpen(false);
    setForm(EMPTY_FORM);
  };

  const onSave = async () => {
    const name = form.name.trim();
    if (!name) {
      error('子智能体名不能为空');
      return;
    }
    if (!form.systemPrompt.trim()) {
      error('系统提示词不能为空');
      return;
    }
    if (form.scope === 'project' && !activeProject?.path) {
      error('项目 scope 需要先打开一个项目');
      return;
    }
    setSaving(true);
    try {
      await invoke('save_subagent', {
        projectPath: activeProject?.path ?? null,
        name,
        description: form.description,
        systemPrompt: form.systemPrompt,
        toolsAllow: parseToolsAllow(form.toolsAllow),
        scope: form.scope,
      });
      success(`已保存子智能体 ${name}`);
      closeForm();
      await reload();
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const onDelete = async (s: SubAgentInfo) => {
    if (!window.confirm(`确认删除子智能体 ${s.name}（${s.sourcePath}）？此操作不可撤销。`)) return;
    try {
      await invoke('delete_subagent', {
        projectPath: activeProject?.path ?? null,
        name: s.name,
        scope: s.scope === 'global' ? 'global' : 'project',
      });
      success(`已删除子智能体 ${s.name}`);
      await reload();
    } catch (e) {
      error(e instanceof Error ? e.message : String(e));
    }
  };

  return (
    <div className="settings-section subagents-section">
      <h3 className="settings-section-title">命名子智能体</h3>
      <p className="settings-section-desc">
        每个子智能体是 <code>.agents/subagents/&lt;name&gt;/AGENT.md</code>。内核构建 agent 时加载，
        主智能体可通过 <code>dispatch_subagent {'{subagent: "<name>"}'}</code> 按名委托。
        <strong> tools_allow</strong> 限制子智能体可用工具（前缀，逗号分隔；留空=继承完整只读工具集）。
        新建/编辑的子智能体在<strong>下次会话提交</strong>时可委托。
      </p>

      {!formOpen && (
        <Button variant="primary" onClick={openCreate} style={{ marginBottom: 16 }}>
          + 新建子智能体
        </Button>
      )}

      {formOpen && (
        <div className="memory-card" style={{ marginBottom: 16 }}>
          <div className="memory-card-header">
            <span className="memory-card-title">
              {form.originalName ? `编辑：${form.originalName}` : '新建子智能体'}
            </span>
          </div>
          <div style={{ display: 'flex', flexDirection: 'column', gap: 10, marginTop: 8 }}>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">
                名称（仅字母/数字/-/_，编辑时不可改名）
              </span>
              <input
                className="provider-input"
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
                placeholder="例如 researcher"
                autoFocus
                disabled={!!form.originalName}
              />
            </label>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">描述</span>
              <input
                className="provider-input"
                value={form.description}
                onChange={(e) => setForm({ ...form, description: e.target.value })}
                placeholder="深度网络调研"
              />
            </label>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">系统提示词（system_prompt）</span>
              <textarea
                className="provider-input"
                value={form.systemPrompt}
                onChange={(e) => setForm({ ...form, systemPrompt: e.target.value })}
                placeholder={'你是调研专家，只给结论不要过程。'}
                rows={4}
                style={{ fontFamily: 'monospace' }}
              />
            </label>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">tools_allow（逗号分隔的工具名前缀，留空=继承全部只读工具）</span>
              <input
                className="provider-input"
                value={form.toolsAllow}
                onChange={(e) => setForm({ ...form, toolsAllow: e.target.value })}
                placeholder="skill__web_search, read_file"
                style={{ fontFamily: 'monospace' }}
              />
            </label>
            <label style={{ display: 'flex', flexDirection: 'column', gap: 4 }}>
              <span className="settings-section-desc">作用域</span>
              <select
                className="provider-input"
                value={form.scope}
                onChange={(e) => setForm({ ...form, scope: e.target.value as SubAgentScope })}
              >
                <option value="project">项目（&lt;项目&gt;/.agents/subagents，随仓库版本化）</option>
                <option value="global">全局（~/.agents/subagents，跨项目共享）</option>
              </select>
            </label>
            <div style={{ display: 'flex', gap: 8 }}>
              <Button variant="primary" onClick={onSave} disabled={saving}>
                {saving ? '保存中…' : '保存'}
              </Button>
              <Button variant="secondary" onClick={closeForm} disabled={saving}>
                取消
              </Button>
            </div>
          </div>
        </div>
      )}

      <div className="skills-subhead">全部子智能体（{agents.length}）</div>
      {loading && agents.length === 0 && <p className="settings-section-desc">加载中...</p>}
      {!loading && agents.length === 0 && (
        <div className="memory-empty">
          <p>暂无子智能体</p>
        </div>
      )}
      <div className="memory-list">
        {agents.map((s) => {
          // app-private is the legacy read-only tier (~/.dev-workbench/subagents):
          // the kernel loads it for dispatch, but the UI must NOT offer edit/delete —
          // save/delete only resolve scope to global/project, so acting on an
          // app-private row would either fail ("子智能体 X 不存在") or silently shadow
          // it with a project copy. Treat it as read-only, matching scope_dir's contract.
          const isReadOnly = s.scope === 'app-private';
          return (
            <div key={`${s.scope}:${s.name}`} className="memory-card skills-card">
              <div className="memory-card-header">
                <span className="memory-card-title">{s.name}</span>
                <span className="memory-card-category">{SCOPE_LABEL[s.scope] ?? s.scope}</span>
              </div>
              {s.description && <p className="memory-card-content">{s.description}</p>}
              <p className="memory-card-content" style={{ fontFamily: 'monospace', fontSize: 12, whiteSpace: 'pre-wrap' }}>
                {s.systemPrompt}
              </p>
              {s.toolsAllow.length > 0 && (
                <p className="settings-section-desc" style={{ fontSize: 12, margin: '4px 0' }}>
                  tools: {s.toolsAllow.join(', ')}
                </p>
              )}
              <p className="settings-section-desc" style={{ fontSize: 11, margin: '4px 0' }} title={s.sourcePath}>
                {s.sourcePath}
              </p>
              <div className="memory-card-meta">
                {isReadOnly ? (
                  <span className="settings-section-desc" style={{ fontSize: 12 }}>内置/只读（不可编辑）</span>
                ) : (
                  <>
                    <Button variant="ghost" size="sm" onClick={() => openEdit(s)} aria-label={`编辑子智能体 ${s.name}`}>
                      编辑
                    </Button>
                    <Button variant="dangerGhost" size="sm" onClick={() => onDelete(s)} aria-label={`删除子智能体 ${s.name}`}>
                      删除
                    </Button>
                  </>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
