import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ProvidersSection } from '../ProvidersSection';
import { useProvidersStore } from '../../../stores/providersStore';
import type { ProvidersConfig } from '../../../types';

/**
 * ProvidersSection drives the global providers.toml: it loads on mount, edits a
 * local draft, persists the whole file on save, and probes credentials on test.
 * These tests stub the Tauri invoke bridge (routed by command name) + Toast so
 * the component renders deterministically and its invoke calls are assertable.
 */
const mockInvoke = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));
vi.mock('../../Toast', () => ({
  useToast: () => ({ info: vi.fn(), error: vi.fn(), success: vi.fn(), toast: vi.fn() }),
}));

const MOCK_CONFIG: ProvidersConfig = {
  providers: [
    {
      id: 'zai',
      name: 'Z.AI (GLM)',
      endpoint: 'https://open.bigmodel.cn/api/anthropic',
      apiKey: 'sk-test-key',
      enabled: true,
      models: [
        { id: 'glm-4.6', label: 'GLM-4.6', enabled: true },
      ],
    },
  ],
  modelMapping: {},
};

function setupInvoke(
  overrides: Partial<Record<string, (args?: unknown) => unknown>> = {},
) {
  mockInvoke.mockImplementation((cmd: string, args?: unknown) => {
    if (cmd === 'get_providers_config') {
      return (overrides.get_providers_config ?? (() => MOCK_CONFIG))(args);
    }
    if (cmd === 'set_providers_config') {
      return (overrides.set_providers_config ?? (() => undefined))(args);
    }
    if (cmd === 'test_provider_connection') {
      return (overrides.test_provider_connection ??
        (() => ({ ok: true, status: 200, message: '连接成功' })))(args);
    }
    return Promise.reject(new Error(`unexpected invoke: ${cmd}`));
  });
}

describe('ProvidersSection', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    // Zustand store is a module singleton — reset between tests so a prior
    // test's loaded config doesn't leak into the next (e.g. the load-failure
    // test must see config === null, not the previous MOCK_CONFIG).
    useProvidersStore.setState({ config: null, loading: false });
  });

  it('loads providers on mount and renders the provider card', async () => {
    setupInvoke();
    render(<ProvidersSection />);
    await waitFor(() => {
      expect(mockInvoke).toHaveBeenCalledWith('get_providers_config');
    });
    expect(await screen.findByText('Z.AI (GLM)')).toBeInTheDocument();
    // Models render as labels.
    expect(screen.getByText(/GLM-4\.6/)).toBeInTheDocument();
  });

  it('edits the endpoint and persists the whole config on save', async () => {
    const user = userEvent.setup();
    let savedConfig: ProvidersConfig | null = null;
    setupInvoke({
      set_providers_config: (args) => {
        savedConfig = (args as { config: ProvidersConfig }).config;
        return undefined;
      },
    });
    render(<ProvidersSection />);
    const endpointInput = await screen.findByPlaceholderText('https://...');
    await user.clear(endpointInput);
    await user.type(endpointInput, 'https://new.endpoint/v1');

    await user.click(screen.getByRole('button', { name: '保存' }));

    await waitFor(() => {
      expect(savedConfig).not.toBeNull();
    });
    expect(savedConfig!.providers[0].endpoint).toBe('https://new.endpoint/v1');
    // The api_key + enabled flags survive the round-trip.
    expect(savedConfig!.providers[0].apiKey).toBe('sk-test-key');
    expect(savedConfig!.providers[0].enabled).toBe(true);
  });

  it('probes credentials with endpoint + key + first enabled model', async () => {
    const user = userEvent.setup();
    let probeArgs: Record<string, unknown> | null = null;
    setupInvoke({
      test_provider_connection: (args) => {
        probeArgs = args as Record<string, unknown>;
        return { ok: true, status: 200, message: '连接成功' };
      },
    });
    render(<ProvidersSection />);
    await screen.findByText('Z.AI (GLM)');
    await user.click(screen.getByRole('button', { name: '测试连接' }));

    await waitFor(() => {
      expect(probeArgs).not.toBeNull();
    });
    expect(probeArgs).toEqual({
      endpoint: 'https://open.bigmodel.cn/api/anthropic',
      apiKey: 'sk-test-key',
      model: 'glm-4.6',
    });
  });

  it('shows the disabled state when the load fails', async () => {
    mockInvoke.mockImplementation(() =>
      Promise.reject(new Error('data dir locked')),
    );
    render(<ProvidersSection />);
    expect(
      await screen.findByText('无法加载供应商配置,请检查应用数据目录权限'),
    ).toBeInTheDocument();
  });
});
