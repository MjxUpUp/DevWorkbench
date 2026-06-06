import { useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { open } from '@tauri-apps/plugin-dialog';
import type { GitRepo, Project } from '../types';

interface AddProjectProps {
  onAdd: (project: Omit<Project, 'id' | 'open_count' | 'last_opened_at' | 'created_at' | 'starred'>) => Promise<void>;
  onClose: () => void;
}

export function AddProject({ onAdd, onClose }: AddProjectProps) {
  const [mode, setMode] = useState<'manual' | 'scan'>('manual');
  const [name, setName] = useState('');
  const [description, setDescription] = useState('');
  const [path, setPath] = useState('');
  const [tags, setTags] = useState('');
  const [scanDir, setScanDir] = useState('');
  const [scanResults, setScanResults] = useState<GitRepo[]>([]);
  const [selectedRepos, setSelectedRepos] = useState<Set<string>>(new Set());
  const [scanning, setScanning] = useState(false);

  const pickDirectory = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected && typeof selected === 'string') {
      setPath(selected);
      if (!name) {
        setName(selected.split(/[/\\]/).filter(Boolean).pop() || '');
      }
    }
  };

  const pickScanDir = async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected && typeof selected === 'string') {
      setScanDir(selected);
    }
  };

  const scan = async () => {
    if (!scanDir) return;
    setScanning(true);
    try {
      const repos = await invoke<GitRepo[]>('scan_git_repos', { rootPath: scanDir, maxDepth: 3 });
      setScanResults(repos);
    } catch (e) {
      console.error('扫描失败:', e);
    } finally {
      setScanning(false);
    }
  };

  const toggleRepo = (repoPath: string) => {
    setSelectedRepos(prev => {
      const next = new Set(prev);
      if (next.has(repoPath)) next.delete(repoPath);
      else next.add(repoPath);
      return next;
    });
  };

  const addManual = async () => {
    if (!name || !path) return;
    await onAdd({
      name,
      description,
      path,
      tags: tags.split(',').map(t => t.trim()).filter(Boolean),
      cover_image: null,
    });
    onClose();
  };

  const addScanned = async () => {
    for (const repo of scanResults) {
      if (selectedRepos.has(repo.path)) {
        await onAdd({
          name: repo.name,
          description: '',
          path: repo.path,
          tags: [],
          cover_image: null,
        });
      }
    }
    onClose();
  };

  return (
    <div className="modal-overlay" onClick={onClose}>
      <div className="modal" onClick={e => e.stopPropagation()}>
        <div className="modal-header">
          <h2>添加项目</h2>
          <button className="modal-close" onClick={onClose}>✕</button>
        </div>

        <div className="modal-tabs">
          <button className={`tab ${mode === 'manual' ? 'active' : ''}`} onClick={() => setMode('manual')}>手动添加</button>
          <button className={`tab ${mode === 'scan' ? 'active' : ''}`} onClick={() => setMode('scan')}>扫描目录</button>
        </div>

        {mode === 'manual' ? (
          <div className="modal-body">
            <label>项目名称 *</label>
            <input value={name} onChange={e => setName(e.target.value)} placeholder="My Project" />

            <label>项目路径 *</label>
            <div className="input-row">
              <input value={path} onChange={e => setPath(e.target.value)} placeholder="/path/to/project" />
              <button onClick={pickDirectory}>选择目录</button>
            </div>

            <label>描述</label>
            <textarea value={description} onChange={e => setDescription(e.target.value)} placeholder="项目简介..." rows={2} />

            <label>标签（逗号分隔）</label>
            <input value={tags} onChange={e => setTags(e.target.value)} placeholder="React, Rust, CLI" />

            <button className="primary-btn" onClick={addManual} disabled={!name || !path}>添加</button>
          </div>
        ) : (
          <div className="modal-body">
            <label>扫描目录</label>
            <div className="input-row">
              <input value={scanDir} onChange={e => setScanDir(e.target.value)} placeholder="选择要扫描的根目录" />
              <button onClick={pickScanDir}>选择</button>
              <button onClick={scan} disabled={!scanDir || scanning}>{scanning ? '扫描中...' : '扫描'}</button>
            </div>

            {scanResults.length > 0 && (
              <div className="scan-results">
                <p>找到 {scanResults.length} 个 Git 仓库：</p>
                {scanResults.map(repo => (
                  <label key={repo.path} className="scan-item">
                    <input
                      type="checkbox"
                      checked={selectedRepos.has(repo.path)}
                      onChange={() => toggleRepo(repo.path)}
                    />
                    <span className="scan-item-name">{repo.name}</span>
                    <span className="scan-item-path">{repo.path}</span>
                  </label>
                ))}
                <button className="primary-btn" onClick={addScanned} disabled={selectedRepos.size === 0}>
                  添加选中的 {selectedRepos.size} 个项目
                </button>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
