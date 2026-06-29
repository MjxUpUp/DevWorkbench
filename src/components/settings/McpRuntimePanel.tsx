import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Button } from '../ui/Button/Button';
import type { McpServerConfig, McpToolListing } from '../../types';

/**
 * MCP 运行时管理面板 (B3)。
 *
 * McpSection 负责 MCP **配置**编辑（草稿→保存→应用，写 mcp-servers.toml），本面板
 * 补的是「运行时」：用户在 UI 里即时连接/断开某个 server、查看其在线状态、浏览并
 * 试跑它暴露的工具——不必改配置文件再重启。
 *
 * 接通的后端命令（前端此前零调用）：
 * - mcp_servers()          → 当前在线 server 名单（连接状态徽标）
 * - mcp_connect/disconnect → 即时连接/断开（纯运行时，不写配置）
 * - mcp_catalog()          → 已连接 server 的工具列表（浏览）
 * - mcp_call_tool()        → 试跑单个工具（参数 + 结果）
 *
 * 连接/断开是运行时操作（不持久化）；持久化由 McpSection 保存/应用负责（apply 时
 * mcp_load_enabled 按配置重连）。两者正交，互不干扰。
 *
 * env 参数：后端 `Option<Vec<(String,String)>>`（tuple 序列化为 [k,v] 数组），
 * 前端 McpServerConfig.env 是 Record<string,string>，连接时转换成 [[k,v],...]。
 */
export function McpRuntimePanel({ servers }: { servers: McpServerConfig[] }) {
  const [connected, setConnected] = useState<string[]>([]);
  const [catalog, setCatalog] = useState<McpToolListing[]>([]);
  const [expandedServer, setExpandedServer] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [params, setParams] = useState<Record<string, string>>({});
  const [results, setResults] = useState<Record<string, unknown>>({});
  const [error, setError] = useState<string | null>(null);

  const refresh = async () => {
    setBusy('refresh');
    setError(null);
    try {
      // 并行拉在线名单 + 工具目录；任一失败 best-effort 返回空（e2e mock / 无连接时）
      const [names, tools] = await Promise.all([
        invoke<string[]>('mcp_servers').catch(() => [] as string[]),
        invoke<McpToolListing[]>('mcp_catalog').catch(() => [] as McpToolListing[]),
      ]);
      setConnected(Array.isArray(names) ? names : []);
      setCatalog(Array.isArray(tools) ? tools : []);
    } finally {
      setBusy(null);
    }
  };

  useEffect(() => {
    void refresh();
  }, []);

  const handleConnect = async (s: McpServerConfig) => {
    setBusy(`connect:${s.name}`);
    setError(null);
    try {
      const env = Object.entries(s.env).map(([k, v]) => [k, v]);
      await invoke('mcp_connect', { name: s.name, command: s.command, args: s.args, env });
      await refresh();
    } catch (e) {
      setError(`连接 ${s.name} 失败：${String(e)}`);
    } finally {
      setBusy(null);
    }
  };

  const handleDisconnect = async (name: string) => {
    setBusy(`disconnect:${name}`);
    setError(null);
    try {
      await invoke('mcp_disconnect', { name });
      await refresh();
    } catch (e) {
      setError(`断开 ${name} 失败：${String(e)}`);
    } finally {
      setBusy(null);
    }
  };

  const handleCall = async (server: string, tool: string) => {
    const key = `${server}/${tool}`;
    setBusy(`call:${key}`);
    setError(null);
    try {
      const raw = params[key]?.trim();
      const args = raw ? JSON.parse(raw) : {};
      const result = await invoke('mcp_call_tool', { serverName: server, toolName: tool, arguments: args });
      setResults((prev) => ({ ...prev, [key]: result }));
    } catch (e) {
      setError(`调用 ${tool} 失败：${String(e)}`);
    } finally {
      setBusy(null);
    }
  };

  if (servers.length === 0) {
    return null;
  }

  return (
    <div className="mcp-runtime">
      <div className="mcp-runtime-head">
        <h4 className="mcp-runtime-title">运行时管理</h4>
        <Button variant="secondary" size="sm" isLoading={busy === 'refresh'} onClick={refresh}>
          ↻ 刷新状态
        </Button>
      </div>
      <p className="settings-section-desc">
        即时连接/断开 server、浏览并试跑工具（无需改配置重启）
      </p>
      {error && <div className="config-center-error">{error}</div>}

      <ul className="mcp-runtime-list">
        {servers.map((s) => {
          const isOnline = connected.includes(s.name);
          const tools = catalog.filter((t) => t.server === s.name);
          const expanded = expandedServer === s.name;
          return (
            <li key={s.name} className="mcp-runtime-item">
              <div className="mcp-runtime-row">
                <span className={`mcp-status-dot ${isOnline ? 'online' : 'offline'}`} />
                <code className="mcp-runtime-name">{s.name}</code>
                <span className="mcp-runtime-status">{isOnline ? '已连接' : '未连接'}</span>
                <div className="mcp-runtime-actions">
                  {isOnline ? (
                    <>
                      <Button
                        variant="secondary"
                        size="sm"
                        disabled={busy !== null}
                        onClick={() => setExpandedServer(expanded ? null : s.name)}
                      >
                        {expanded ? '收起工具' : `工具（${tools.length}）`}
                      </Button>
                      <Button
                        variant="dangerGhost"
                        size="sm"
                        isLoading={busy === `disconnect:${s.name}`}
                        disabled={busy !== null && busy !== `disconnect:${s.name}`}
                        onClick={() => handleDisconnect(s.name)}
                      >
                        断开
                      </Button>
                    </>
                  ) : (
                    <Button
                      variant="primary"
                      size="sm"
                      isLoading={busy === `connect:${s.name}`}
                      disabled={busy !== null && busy !== `connect:${s.name}`}
                      onClick={() => handleConnect(s)}
                    >
                      连接
                    </Button>
                  )}
                </div>
              </div>

              {expanded && isOnline && (
                <div className="mcp-tools">
                  {tools.length === 0 ? (
                    <p className="muted">该 server 暂无可用工具</p>
                  ) : (
                    tools.map((t) => {
                      const key = `${s.name}/${t.name}`;
                      return (
                        <div key={t.name} className="mcp-tool">
                          <div className="mcp-tool-head">
                            <code className="mcp-tool-name">{t.name}</code>
                            <span className="mcp-tool-desc">{t.description || '(无描述)'}</span>
                          </div>
                          <div className="mcp-tool-call">
                            <input
                              className="mcp-tool-input"
                              type="text"
                              placeholder='参数 JSON，如 {"path": "/tmp"}'
                              value={params[key] ?? ''}
                              onChange={(e) => setParams((p) => ({ ...p, [key]: e.target.value }))}
                            />
                            <Button
                              variant="secondary"
                              size="sm"
                              isLoading={busy === `call:${key}`}
                              onClick={() => handleCall(s.name, t.name)}
                            >
                              调用
                            </Button>
                          </div>
                          {results[key] !== undefined && (
                            <pre className="mcp-tool-result">{JSON.stringify(results[key], null, 2)}</pre>
                          )}
                        </div>
                      );
                    })
                  )}
                </div>
              )}
            </li>
          );
        })}
      </ul>
    </div>
  );
}
