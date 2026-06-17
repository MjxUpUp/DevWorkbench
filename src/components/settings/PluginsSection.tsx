import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useNavigationStore } from '../../stores/navigationStore';
import { useSkillsStore } from '../../stores/skillsStore';
import type { McpToolListing } from '../../types';

/**
 * 能力总览 (D1 — the "plugins" settings page).
 *
 * A READ-ONLY aggregate of everything a kernel agent can currently do, in one
 * place:
 *  - built-in tools (read_file/glob/grep/bash/write_file/dispatch_subagent) —
 *    the agent's baseline faculties, listed for visibility. NOT toggleable:
 *    these are non-negotiable (disabling bash/write_file would brick the agent,
 *    disabling read_file/glob/grep would blind it).
 *  - installed Skills (reuses useSkillsStore.installed — the SAME source
 *    SkillsSection reads, so no duplicate fetch and the two views never drift).
 *  - connected MCP tools (mcp_catalog — live tools across every MCP server).
 *
 * Distinct from the Skills / MCP CONFIG pages (their own sidebar entries):
 * those EDIT the registries; this one answers "what can my agent use right
 * now?" without making the user click through three pages.
 */

// Core built-in tools every kernel agent gets. Mirrors kernel_impl::builtin_tools
// + the dispatch_subagent sub-agent tool. Kept in sync manually — if a new
// built-in tool is added in Rust, add it here so the overview stays honest.
const BUILTIN_TOOLS: { name: string; desc: string }[] = [
  { name: 'read_file', desc: '读取文件内容' },
  { name: 'glob', desc: '按 glob 模式查找文件' },
  { name: 'grep', desc: '在文件内容中搜索（正则）' },
  { name: 'bash', desc: '执行 shell 命令' },
  { name: 'write_file', desc: '写入或修改文件' },
  { name: 'dispatch_subagent', desc: '把子任务派给命名 / 匿名子 agent' },
];

interface ToolRow {
  name: string;
  desc: string;
}

export function PluginsSection() {
  const activeProject = useNavigationStore((s) => s.activeProject);
  const installed = useSkillsStore((s) => s.installed);
  const loadInstalled = useSkillsStore((s) => s.loadInstalled);
  const [mcpTools, setMcpTools] = useState<McpToolListing[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    loadInstalled();
    let cancelled = false;
    (async () => {
      try {
        // mcp_catalog needs live MCP connections. The IPC may resolve to null
        // when un-mocked (e2e harness returns null for unknown cmds) or to a
        // non-array on a misbehaving server — coerce to [] so the render's
        // .map never throws. .catch alone isn't enough: it only handles reject,
        // not a fulfilled null.
        const raw = await invoke<McpToolListing[]>('mcp_catalog').catch(
          () => [] as McpToolListing[],
        );
        const m: McpToolListing[] = Array.isArray(raw) ? raw : [];
        if (!cancelled) setMcpTools(m);
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [loadInstalled, activeProject]);

  const skillRows: ToolRow[] = installed.map((s) => ({
    name: s.name,
    desc: s.description ?? '(无描述)',
  }));
  const mcpRows: ToolRow[] = mcpTools.map((t) => ({
    name: `${t.server} / ${t.name}`,
    desc: t.description || '(无描述)',
  }));

  return (
    <div className="settings-section">
      <h3 className="settings-section-title">能力总览</h3>
      <p className="settings-section-desc">
        {activeProject ? (
          <>
            项目 <code>{activeProject.name}</code> 的 agent 当前可用能力（只读总览；增删请用左侧"技能 /
            MCP 服务器"）
          </>
        ) : (
          '未选项目时仅显示内置工具；选择项目后展示该项目的 Skills 与 MCP 工具'
        )}
      </p>

      {loading && <div className="config-center-loading">加载中...</div>}

      <ToolGroup title="内置工具" rows={BUILTIN_TOOLS} />
      <ToolGroup title={`Skills（${skillRows.length}）`} rows={skillRows} empty="暂无已安装 Skill" />
      <ToolGroup
        title={`MCP 工具（${mcpRows.length}）`}
        rows={mcpRows}
        empty="暂无已连接 MCP 工具（在 MCP 服务器页连接后此处可见）"
      />
    </div>
  );
}

function ToolGroup({
  title,
  rows,
  empty,
}: {
  title: string;
  rows: ToolRow[];
  empty?: string;
}) {
  return (
    <div className="capability-group">
      <h4 className="capability-group-title">{title}</h4>
      {rows.length === 0 ? (
        <p className="muted">{empty ?? '暂无'}</p>
      ) : (
        <ul className="capability-list">
          {rows.map((r) => (
            <li key={r.name} className="capability-item">
              <code className="capability-name">{r.name}</code>
              <span className="capability-desc">{r.desc}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
