import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ProvidersSection } from '../ProvidersSection';
import { useProvidersStore } from '../../../stores/providersStore';
import type { ProvidersConfig } from '../../../types';

/**
 * ProvidersSection drives the global providers.toml. These tests stub the Tauri
 * invoke bridge (routed by command name) + Toast so renders are deterministic
 * and invoke calls + toast feedback are assertable. They cover the UX redesign:
 * section-level save (not per-card), dirty indicator, validation, key reveal,
 * in-card test result, add/remove provider+model, and default-model mapping.
 *
 * Note: MOCK_CONFIG has 2 providers, so per-card controls (endpoint/key/test
 * button) are scoped with `within(card)` to disambiguate.
 */
const mockInvoke = vi.hoisted(() => vi.fn());
const toastSpies = vi.hoisted(() => ({
  info: vi.fn(),
  error: vi.fn(),
  success: vi.fn(),
  toast: vi.fn(),
}));
vi.mock('@tauri-apps/api/core', () => ({ invoke: mockInvoke }));
vi.mock('../../Toast', () => ({
  useToast: () => toastSpies,
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
        { id: 'glm-4-plus', label: 'GLM-4-Plus', enabled: true },
      ],
    },
    {
      id: 'anthropic',
      name: 'Anthropic',
      endpoint: 'https://api.anthropic.com',
      apiKey: '',
      enabled: false,
      models: [{ id: 'claude-sonnet-4-6', label: 'Claude Sonnet 4.6', enabled: true }],
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
      return (
        overrides.test_provider_connection ??
        (() => ({ ok: true, status: 200, message: '连接成功' }))
      )(args);
    }
    return Promise.reject(new Error(`unexpected invoke: ${cmd}`));
  });
}

/** Resolve the Z.AI card element after config has loaded. */
async function zaiCard(): Promise<HTMLElement> {
  const nameInput = await screen.findByDisplayValue('Z.AI (GLM)');
  return nameInput.closest('.provider-card') as HTMLElement;
}

describe('ProvidersSection', () => {
  beforeEach(() => {
    mockInvoke.mockReset();
    Object.values(toastSpies).forEach((s) => s.mockClear());
    // Zustand store is a module singleton — reset between tests so a prior
    // test's loaded config doesn't leak (e.g. load-failure must see null).
    useProvidersStore.setState({ config: null, loading: false });
  });

  it('loads providers on mount and renders provider cards', async () => {
    setupInvoke();
    render(<ProvidersSection />);
    const zai = await zaiCard();
    expect(screen.getByDisplayValue('Anthropic')).toBeInTheDocument();
    // Models render as editable rows (label inputs). (Can't use getByDisplayValue
    // here: testing-library matches a <select> against its selected option's text,
    // and the in-card test-model select also shows "GLM-4.6".)
    const labels = within(zai).getAllByLabelText('模型显示名');
    expect(labels.map((i) => (i as HTMLInputElement).value)).toEqual([
      'GLM-4.6',
      'GLM-4-Plus',
    ]);
  });

  it('persists the whole config via the section-level save (not per-card)', async () => {
    const user = userEvent.setup();
    let savedConfig: ProvidersConfig | null = null;
    setupInvoke({
      set_providers_config: (args) => {
        savedConfig = (args as { config: ProvidersConfig }).config;
        return undefined;
      },
    });
    render(<ProvidersSection />);
    const zai = await zaiCard();

    // No per-card save buttons — only one section-level save.
    expect(screen.getAllByRole('button', { name: /保存全部更改|已保存/ })).toHaveLength(1);

    const endpointInput = within(zai).getByLabelText('接口地址');
    await user.clear(endpointInput);
    await user.type(endpointInput, 'https://new.endpoint/v1');

    await user.click(screen.getByRole('button', { name: '保存全部更改' }));

    await waitFor(() => expect(savedConfig).not.toBeNull());
    expect(savedConfig!.providers[0].endpoint).toBe('https://new.endpoint/v1');
    // Other provider + api_key survive the round-trip.
    expect(savedConfig!.providers[0].apiKey).toBe('sk-test-key');
    expect(savedConfig!.providers[1].name).toBe('Anthropic');
  });

  it('shows a dirty badge while editing and clears it after save', async () => {
    const user = userEvent.setup();
    setupInvoke();
    render(<ProvidersSection />);
    const zai = await zaiCard();

    // Clean on load — no dirty badge.
    expect(screen.queryByText('有未保存的更改')).not.toBeInTheDocument();

    await user.type(within(zai).getByLabelText('接口地址'), '-edited');
    expect(await screen.findByText('有未保存的更改')).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: '保存全部更改' }));
    await waitFor(() => {
      expect(screen.queryByText('有未保存的更改')).not.toBeInTheDocument();
    });
  });

  it('blocks save and flags the field when an enabled provider has an invalid endpoint', async () => {
    const user = userEvent.setup();
    setupInvoke();
    render(<ProvidersSection />);
    const zai = await zaiCard();

    const endpointInput = within(zai).getByLabelText('接口地址');
    await user.clear(endpointInput);
    await user.type(endpointInput, 'not-a-url');

    await user.click(screen.getByRole('button', { name: '保存全部更改' }));

    // Validation error surfaces in-card and the persist call never fires.
    expect(await within(zai).findByText('需为合法 http(s) 地址')).toBeInTheDocument();
    expect(mockInvoke).not.toHaveBeenCalledWith('set_providers_config', expect.anything());
    expect(toastSpies.error).toHaveBeenCalledWith('请修正标红的字段后再保存');
  });

  it('toggles API Key visibility', async () => {
    const user = userEvent.setup();
    setupInvoke();
    render(<ProvidersSection />);
    const zai = await zaiCard();

    const keyInput = within(zai).getByLabelText('API Key');
    expect(keyInput).toHaveAttribute('type', 'password');

    await user.click(within(zai).getByRole('button', { name: '显示密钥' }));
    expect(keyInput).toHaveAttribute('type', 'text');

    await user.click(within(zai).getByRole('button', { name: '隐藏密钥' }));
    expect(keyInput).toHaveAttribute('type', 'password');
  });

  it('shows the in-card test result after probing (not just a toast)', async () => {
    const user = userEvent.setup();
    setupInvoke({
      test_provider_connection: () => ({ ok: false, status: 401, message: 'invalid api key' }),
    });
    render(<ProvidersSection />);
    const zai = await zaiCard();

    await user.click(within(zai).getByRole('button', { name: '测试连接' }));

    // Failure result persists in-card with the message.
    expect(await within(zai).findByText(/invalid api key/)).toBeInTheDocument();
  });

  it('probes credentials with endpoint + key + the selected model', async () => {
    const user = userEvent.setup();
    let probeArgs: Record<string, unknown> | null = null;
    setupInvoke({
      test_provider_connection: (args) => {
        probeArgs = args as Record<string, unknown>;
        return { ok: true, status: 200, message: 'ok' };
      },
    });
    render(<ProvidersSection />);
    const zai = await zaiCard();

    // Pick the second model, then probe.
    await user.selectOptions(within(zai).getByLabelText('测试模型 - Z.AI (GLM)'), 'glm-4-plus');
    await user.click(within(zai).getByRole('button', { name: '测试连接' }));

    await waitFor(() => expect(probeArgs).not.toBeNull());
    expect(probeArgs).toEqual({
      endpoint: 'https://open.bigmodel.cn/api/anthropic',
      apiKey: 'sk-test-key',
      model: 'glm-4-plus',
    });
  });

  it('enables the test button for a freshly-added provider once a model lands (testModel sync)', async () => {
    // Regression: a provider created in-session mounts its card with models=[]
    // → useState initialises testModel='' and stays '' after the user adds a
    // model (key={p.id} is stable, so adding a model re-renders without
    // remounting). That left the 测试连接 button permanently disabled — the
    // "新增的供应商没办法测试链接" symptom. The useEffect now re-syncs testModel
    // to the first enabled model when the list changes.
    const user = userEvent.setup();
    let probeArgs: Record<string, unknown> | null = null;
    setupInvoke({
      test_provider_connection: (args) => {
        probeArgs = args as Record<string, unknown>;
        return { ok: true, status: 200, message: 'ok' };
      },
    });
    render(<ProvidersSection />);
    await zaiCard();

    await user.click(screen.getByRole('button', { name: '+ 添加供应商' }));
    const newCard = screen.getByDisplayValue('新供应商').closest('.provider-card') as HTMLElement;

    // Before any model: testModel='' → button disabled.
    const testBtn = within(newCard).getByRole('button', { name: '测试连接' });
    expect(testBtn).toBeDisabled();

    // Add a model row and name its id.
    await user.click(within(newCard).getByRole('button', { name: '+ 添加模型' }));
    const idInputs = within(newCard).getAllByLabelText('模型 ID');
    await user.clear(idInputs[0]);
    await user.type(idInputs[0], 'fresh-model');

    // testModel syncs to the freshly-added model → button enabled.
    await waitFor(() => expect(testBtn).toBeEnabled());

    // Probing carries the synced model id, not the stale ''.
    await user.click(testBtn);
    await waitFor(() => expect(probeArgs).not.toBeNull());
    expect(probeArgs!.model).toBe('fresh-model');
  });

  it('adds a new provider card on demand', async () => {
    const user = userEvent.setup();
    setupInvoke();
    render(<ProvidersSection />);
    await zaiCard();

    expect(screen.getAllByLabelText('供应商名称')).toHaveLength(2);
    await user.click(screen.getByRole('button', { name: '+ 添加供应商' }));
    expect(screen.getAllByLabelText('供应商名称')).toHaveLength(3);
    expect(screen.getByDisplayValue('新供应商')).toBeInTheDocument();
  });

  it('removes a provider card', async () => {
    const user = userEvent.setup();
    setupInvoke();
    render(<ProvidersSection />);
    await zaiCard();

    expect(screen.getByDisplayValue('Anthropic')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: '删除供应商 Anthropic' }));
    expect(screen.queryByDisplayValue('Anthropic')).not.toBeInTheDocument();
  });

  it('adds a model row to a provider', async () => {
    const user = userEvent.setup();
    setupInvoke();
    render(<ProvidersSection />);
    const zai = await zaiCard();

    expect(within(zai).getAllByLabelText('模型显示名')).toHaveLength(2);
    await user.click(within(zai).getByRole('button', { name: '+ 添加模型' }));
    expect(within(zai).getAllByLabelText('模型显示名')).toHaveLength(3);
  });

  it('writes the default-model selection into modelMapping on save', async () => {
    const user = userEvent.setup();
    let savedConfig: ProvidersConfig | null = null;
    setupInvoke({
      set_providers_config: (args) => {
        savedConfig = (args as { config: ProvidersConfig }).config;
        return undefined;
      },
    });
    render(<ProvidersSection />);
    await zaiCard();

    // Redirect the kernel default (glm-4.6) to glm-4-plus.
    await user.selectOptions(screen.getByLabelText('默认模型'), 'glm-4-plus');
    await user.click(screen.getByRole('button', { name: '保存全部更改' }));

    await waitFor(() => expect(savedConfig).not.toBeNull());
    expect(savedConfig!.modelMapping['glm-4.6']).toBe('glm-4-plus');
  });

  it('shows the disabled state when the load fails', async () => {
    mockInvoke.mockImplementation(() => Promise.reject(new Error('data dir locked')));
    render(<ProvidersSection />);
    expect(
      await screen.findByText('无法加载供应商配置,请检查应用数据目录权限'),
    ).toBeInTheDocument();
  });

  it('persists a model context window through the save round-trip', async () => {
    const user = userEvent.setup();
    let savedConfig: ProvidersConfig | null = null;
    setupInvoke({
      set_providers_config: (args) => {
        savedConfig = (args as { config: ProvidersConfig }).config;
        return undefined;
      },
    });
    render(<ProvidersSection />);
    const zai = await zaiCard();

    // The first model (glm-4.6) starts with no declared window. Type one in.
    const windowInputs = within(zai).getAllByLabelText('上下文窗口');
    await user.type(windowInputs[0], '200000');

    await user.click(screen.getByRole('button', { name: '保存全部更改' }));

    await waitFor(() => expect(savedConfig).not.toBeNull());
    // The edited model carries the window the backend uses to size compaction.
    expect(savedConfig!.providers[0].models[0].contextWindow).toBe(200000);
    // A model the user left blank stays undefined → backend 32k default.
    expect(savedConfig!.providers[0].models[1].contextWindow).toBeUndefined();
  });
});
