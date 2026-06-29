import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, fireEvent } from '@testing-library/react';
import { McpRuntimePanel } from '../McpRuntimePanel';
import type { McpServerConfig } from '../../../types';

/**
 * McpRuntimePanel 补 B3 断点：MCP 运行时管理命令（mcp_servers/connect/disconnect/
 * catalog/call_tool）前端此前零调用。覆盖：初始状态查询、连接往返、断开往返、
 * 工具试跑往返、无 server 时不渲染。
 */
const mockInvoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));

function makeServer(name: string): McpServerConfig {
  return {
    name,
    command: 'npx',
    args: ['-y', `@mcp/${name}`],
    env: { API_KEY: 'secret' },
    enabled: true,
    targetAgents: [],
  };
}

describe('McpRuntimePanel', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    // 默认安全返回——未设 mockImplementation 的测试（如「无 server 不渲染」），
    // 组件 mount 的 refresh() 仍会调 invoke，需返回 Promise 而非 undefined。
    mockInvoke.mockResolvedValue([]);
  });

  it('无配置 server 时不渲染', () => {
    const { container } = render(<McpRuntimePanel servers={[]} />);
    expect(container.firstChild).toBeNull();
  });

  it('挂载即查 mcp_servers + mcp_catalog，离线 server 显示"连接"按钮', async () => {
    mockInvoke.mockImplementation((cmd: string) => {
      if (cmd === 'mcp_servers') return Promise.resolve([] as string[]);
      if (cmd === 'mcp_catalog') return Promise.resolve([]);
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<McpRuntimePanel servers={[makeServer('filesystem')]} />);

    expect(await screen.findByText('filesystem')).toBeInTheDocument();
    expect(screen.getByText('未连接')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: '连接' })).toBeInTheDocument();
  });

  it('点击"连接"调 mcp_connect（env 转 [[k,v]] 数组）并刷新状态', async () => {
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === 'mcp_servers') {
        // 第1次（挂载 refresh）离线；connect 后第2次（refresh）在线
        const calls = mockInvoke.mock.calls.filter(([c]) => c === 'mcp_servers').length;
        return Promise.resolve(calls <= 1 ? [] : (['filesystem'] as string[]));
      }
      if (cmd === 'mcp_catalog') return Promise.resolve([]);
      if (cmd === 'mcp_connect') {
        expect(args).toEqual({
          name: 'filesystem',
          command: 'npx',
          args: ['-y', '@mcp/filesystem'],
          env: [['API_KEY', 'secret']],
        });
        return Promise.resolve();
      }
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<McpRuntimePanel servers={[makeServer('filesystem')]} />);

    // 初始离线
    await screen.findByText('未连接');
    fireEvent.click(screen.getByRole('button', { name: '连接' }));

    // 连接后刷新 → 在线，显示"断开"
    await waitFor(() => expect(screen.getByText('已连接')).toBeInTheDocument());
    expect(mockInvoke).toHaveBeenCalledWith('mcp_connect', {
      name: 'filesystem',
      command: 'npx',
      args: ['-y', '@mcp/filesystem'],
      env: [['API_KEY', 'secret']],
    });
  });

  it('在线 server 点击"断开"调 mcp_disconnect', async () => {
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === 'mcp_servers') {
        // 第一次（挂载）在线；断开后第二次（刷新）离线
        const calls = mockInvoke.mock.calls.filter(([c]) => c === 'mcp_servers').length;
        return Promise.resolve(calls <= 1 ? ['fs'] : [] as string[]);
      }
      if (cmd === 'mcp_catalog') return Promise.resolve([]);
      if (cmd === 'mcp_disconnect') {
        expect(args).toEqual({ name: 'fs' });
        return Promise.resolve();
      }
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<McpRuntimePanel servers={[makeServer('fs')]} />);

    await screen.findByText('已连接');
    fireEvent.click(screen.getByRole('button', { name: '断开' }));
    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('mcp_disconnect', { name: 'fs' }),
    );
  });

  it('展开工具并调用：mcp_call_tool 带 serverName/toolName/arguments', async () => {
    mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
      if (cmd === 'mcp_servers') return Promise.resolve(['fs'] as string[]);
      if (cmd === 'mcp_catalog') {
        return Promise.resolve([
          { server: 'fs', name: 'read_file', description: '读取文件', inputSchema: {} },
        ]);
      }
      if (cmd === 'mcp_call_tool') {
        expect(args).toEqual({ serverName: 'fs', toolName: 'read_file', arguments: { path: '/x' } });
        return Promise.resolve({ content: 'hello' });
      }
      return Promise.reject(new Error(`unexpected ${cmd}`));
    });
    render(<McpRuntimePanel servers={[makeServer('fs')]} />);

    await screen.findByText('已连接');
    // 展开（按钮文案含工具数）
    fireEvent.click(screen.getByRole('button', { name: /工具/ }));
    expect(await screen.findByText('read_file')).toBeInTheDocument();

    // 输入参数 + 调用
    const input = screen.getByPlaceholderText(/参数 JSON/);
    fireEvent.change(input, { target: { value: '{"path": "/x"}' } });
    fireEvent.click(screen.getByRole('button', { name: '调用' }));

    await waitFor(() =>
      expect(mockInvoke).toHaveBeenCalledWith('mcp_call_tool', {
        serverName: 'fs',
        toolName: 'read_file',
        arguments: { path: '/x' },
      }),
    );
    // 结果渲染
    await waitFor(() => expect(screen.getByText(/hello/)).toBeInTheDocument());
  });
});
