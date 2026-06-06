import { useState } from 'react';
import { IconX } from './Icons';
import type { Project } from '../types';

interface EditProjectProps {
  project: Project;
  onSave: (id: string, updates: Partial<Pick<Project, 'name' | 'description' | 'tags'>>) => Promise<void>;
  onClose: () => void;
}

export function EditProject({ project, onSave, onClose }: EditProjectProps) {
  const [name, setName] = useState(project.name);
  const [description, setDescription] = useState(project.description);
  const [tags, setTags] = useState(project.tags.join(', '));
  const [error, setError] = useState('');
  const [saving, setSaving] = useState(false);

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
          <input value={tags} onChange={e => setTags(e.target.value)} placeholder="React, Rust, CLI" />

          <button className="primary-btn" onClick={handleSave} disabled={!name.trim() || saving}>
            {saving ? '保存中...' : '保存'}
          </button>
        </div>
      </div>
    </div>
  );
}
