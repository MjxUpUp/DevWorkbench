import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { IconX } from './Icons';
import type { Project } from '../types';

const WORKSPACE_TOOLS = [
  { key: 'claude', label: 'Claude' },
  { key: 'cursor', label: 'Cursor' },
  { key: 'code', label: 'VS Code' },
  { key: 'finder', label: 'Files' },
  { key: 'pi', label: 'Pi' },
  { key: 'codex', label: 'Codex' },
] as const;

interface EditProjectProps {
  project: Project;
  onSave: (id: string, updates: Partial<Project>) => Promise<void>;
  onClose: () => void;
}

export function EditProject({ project, onSave, onClose }: EditProjectProps) {
  const [name, setName] = useState(project.name);
  const [description, setDescription] = useState(project.description);
  const [tags, setTags] = useState(project.tags.join(', '));
  const [workspaceTools, setWorkspaceTools] = useState<string[]>(project.workspace_tools);
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);

  const toggleTool = (toolKey: string) => {
    setWorkspaceTools(prev =>
      prev.includes(toolKey)
        ? prev.filter(t => t !== toolKey)
        : [...prev, toolKey]
    );
  };

  const detectTags = async () => {
    try {
      const detected = await invoke<string[]>('detect_project_tags', { projectPath: project.path });
      if (detected.length > 0) {
        setTags(detected.join(', '));
      }
    } catch {
      // 检测失败不影响
    }
  };

  const handleSave = async () => {
    if (!name.trim()) {
      setError('项目名称不能为空');
      return;
    }

    setSaving(true);
    setError('');
    try {
      await onSave(project.id, {
        name: name.trim(),
        description: description.trim(),
        tags: tags.split(',').map(t => t.trim()).filter(Boolean),
        workspace_tools: workspaceTools,
      });
      onClose();
    } catch (e) {
      setError(`保存失败: ${e}`);
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal" onClick={e => e.stopPropagation()}>
        <div className="modal-header">
          <h2>编辑项目</h2>
          <button className="modal-close" onClick={onClose}><IconX size={16} /></button>
        </div>

        {error && <div className="error-banner">{error}</div>}

        <div className="modal-body">
          <label>项目名称</label>
          <input value={name} onChange={e => { setName(e.target.value); setError(''); }} placeholder="项目名称" />

          <label>项目路径</label>
          <input value={project.path} disabled style={{ opacity: 0.5, cursor: 'not-allowed' }} />

          <label>描述</label>
          <textarea value={description} onChange={e => setDescription(e.target.value)} placeholder="项目简介..." rows={2} />

          <label>标签（逗号分隔）</label>
          <div className="input-row">
            <input value={tags} onChange={e => setTags(e.target.value)} placeholder="React, Rust, CLI" />
            <button onClick={detectTags}>检测</button>
          </div>

          <label>工作区工具</label>
          <p className="workspace-tools-hint">选择该项目常用工具，将显示为快捷启动按钮</p>
          <div className="tool-checkbox-group">
            {WORKSPACE_TOOLS.map(({ key, label }) => (
              <label key={key} className={`tool-checkbox ${workspaceTools.includes(key) ? 'checked' : ''}`}>
                <input
                  type="checkbox"
                  checked={workspaceTools.includes(key)}
                  onChange={() => toggleTool(key)}
                />
                <span>{label}</span>
              </label>
            ))}
          </div>

          <button className="primary-btn" onClick={handleSave} disabled={!name.trim() || saving}>
            {saving ? '保存中...' : '保存'}
          </button>
        </div>
      </div>
    </div>
  );
}
