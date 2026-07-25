/** @vitest-environment jsdom */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import type {
  PublicAgentProfile,
  PublicBotAccount,
  PublicMcpProfile,
  PublicOutputChannel,
  PublicProviderConfig,
} from '../../types/wire.ts';
import { SettingsModal } from './SettingsModal.tsx';

const apiMocks = vi.hoisted(() => ({
  listAgentProfiles: vi.fn(),
  listBotAccounts: vi.fn(),
  listMcpProfiles: vi.fn(),
  listOutputChannels: vi.fn(),
  listProviders: vi.fn(),
  putAgentProfile: vi.fn(),
  putBotAccount: vi.fn(),
  putMcpProfile: vi.fn(),
  putOutputChannel: vi.fn(),
  putProvider: vi.fn(),
  testOutputChannel: vi.fn(),
}));

vi.mock('../../api/http.ts', () => apiMocks);

const provider: PublicProviderConfig = {
  profile_id: 'default',
  provider: 'openai_chat',
  api_key_configured: true,
  base_url: 'https://example.test/v1',
  model: 'test-model',
  max_output_tokens: 4096,
  max_context_tokens: 128000,
  temperature: null,
  reasoning_effort: null,
  max_retries: 10,
  request_timeout_secs: 30,
  stream_idle_timeout_secs: 120,
  revision: 1,
};

const anthropicProvider: PublicProviderConfig = {
  ...provider,
  profile_id: 'anthropic-prod',
  provider: 'anthropic',
  base_url: 'https://anthropic.example.test',
  model: 'claude-test',
  revision: 2,
};

const agentProfile: PublicAgentProfile = {
  agent_profile_id: 'reviewer',
  revision: 1,
  prompt: { mode: 'extend', text: 'Review carefully.' },
  tools: { allow: null, deny: [] },
  skills: { allow: null, deny: [] },
  mcp_profile_ids: [],
  initial_capability_mode: 'full_access',
  model: null,
  reasoning_effort: null,
};

const remoteMcpProfile: PublicMcpProfile = {
  mcp_profile_id: 'remote',
  revision: 1,
  transport: {
    type: 'http',
    url: 'https://mcp.example.test/rpc',
    bearer_token_configured: false,
    header_names: [],
    allow_stateless: true,
    reinitialize_on_expired_session: true,
  },
  tool_name_prefix: 'remote',
  connect_timeout_secs: 30,
  request_timeout_secs: 60,
  max_output_lines: 2000,
  max_output_bytes: 51200,
};

describe('SettingsModal', () => {
  beforeEach(() => {
    apiMocks.listProviders.mockResolvedValue({ providers: [provider] });
    apiMocks.listAgentProfiles.mockResolvedValue({
      agent_profiles: [agentProfile],
    });
    apiMocks.listMcpProfiles.mockResolvedValue({
      mcp_profiles: [remoteMcpProfile],
    });
    apiMocks.listBotAccounts.mockResolvedValue({ bot_accounts: [] });
    apiMocks.listOutputChannels.mockResolvedValue({ output_channels: [] });
    apiMocks.putProvider.mockReset();
    apiMocks.putAgentProfile.mockReset();
    apiMocks.putBotAccount.mockReset();
    apiMocks.putMcpProfile.mockReset();
    apiMocks.putOutputChannel.mockReset();
    apiMocks.testOutputChannel.mockReset();
  });

  afterEach(cleanup);

  it('selects an unchanged configured profile without overwriting its API key', async () => {
    const onSaveAuthKey = vi.fn();
    const onSaveProfileId = vi.fn();
    const onConfigured = vi.fn();
    render(
      <I18nProvider initialLocale="en">
        <SettingsModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId=""
          capabilityMode={null}
          onClose={vi.fn()}
          onSaveAuthKey={onSaveAuthKey}
          onSaveProfileId={onSaveProfileId}
          onSaveAgentProfileId={vi.fn()}
          onSaveCapabilityMode={vi.fn()}
          onProviderSaved={vi.fn()}
          onConfigured={onConfigured}
        />
      </I18nProvider>,
    );

    await waitFor(() => {
      expect(screen.getByDisplayValue('test-model')).toBeTruthy();
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => {
      expect(onConfigured).toHaveBeenCalled();
    });
    expect(apiMocks.putProvider).not.toHaveBeenCalled();
    expect(onSaveAuthKey).toHaveBeenCalledWith('daemon-key');
    expect(onSaveProfileId).toHaveBeenCalledWith('default');
  });

  it('requires the provider API key after profile fields change', async () => {
    render(
      <I18nProvider initialLocale="en">
        <SettingsModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId=""
          capabilityMode={null}
          onClose={vi.fn()}
          onSaveAuthKey={vi.fn()}
          onSaveProfileId={vi.fn()}
          onSaveAgentProfileId={vi.fn()}
          onSaveCapabilityMode={vi.fn()}
          onProviderSaved={vi.fn()}
          onConfigured={vi.fn()}
        />
      </I18nProvider>,
    );
    await waitFor(() => {
      expect(screen.getByDisplayValue('test-model')).toBeTruthy();
    });

    fireEvent.change(screen.getByDisplayValue('test-model'), {
      target: { value: 'changed-model' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    expect(
      screen.getByText(
        'Enter the provider API key to create or update this profile.',
      ),
    ).toBeTruthy();
    expect(apiMocks.putProvider).not.toHaveBeenCalled();
  });

  it('saves optional new-session Agent Profile and capability defaults', async () => {
    const onSaveAgentProfileId = vi.fn();
    const onSaveCapabilityMode = vi.fn();
    render(
      <I18nProvider initialLocale="en">
        <SettingsModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId=""
          capabilityMode={null}
          onClose={vi.fn()}
          onSaveAuthKey={vi.fn()}
          onSaveProfileId={vi.fn()}
          onSaveAgentProfileId={onSaveAgentProfileId}
          onSaveCapabilityMode={onSaveCapabilityMode}
          onProviderSaved={vi.fn()}
          onConfigured={vi.fn()}
        />
      </I18nProvider>,
    );
    await waitFor(() => {
      expect(screen.getByDisplayValue('test-model')).toBeTruthy();
    });

    fireEvent.click(
      screen.getByText('New session defaults', { selector: 'summary' }),
    );
    fireEvent.change(screen.getByLabelText('Agent Profile id (optional)'), {
      target: { value: 'reviewer' },
    });
    fireEvent.change(screen.getByLabelText('Capability mode'), {
      target: { value: 'read_only' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => {
      expect(onSaveAgentProfileId).toHaveBeenCalledWith('reviewer');
      expect(onSaveCapabilityMode).toHaveBeenCalledWith('read_only');
    });
    expect(apiMocks.putProvider).not.toHaveBeenCalled();
  });

  it('lists multiple profiles and switches the editor selection', async () => {
    apiMocks.listProviders.mockResolvedValue({
      providers: [provider, anthropicProvider],
    });
    render(
      <I18nProvider initialLocale="en">
        <SettingsModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId=""
          capabilityMode={null}
          onClose={vi.fn()}
          onSaveAuthKey={vi.fn()}
          onSaveProfileId={vi.fn()}
          onSaveAgentProfileId={vi.fn()}
          onSaveCapabilityMode={vi.fn()}
          onProviderSaved={vi.fn()}
          onConfigured={vi.fn()}
        />
      </I18nProvider>,
    );

    await waitFor(() => {
      expect(
        screen.getByRole('button', { name: /anthropic-prod/ }),
      ).toBeTruthy();
    });
    fireEvent.click(screen.getByRole('button', { name: /anthropic-prod/ }));

    expect(screen.getByDisplayValue('claude-test')).toBeTruthy();
    expect(screen.getByText('anthropic-prod', { selector: 'h3' })).toBeTruthy();
  });

  it('creates an additional provider profile without replacing the list', async () => {
    const created = {
      ...anthropicProvider,
      profile_id: 'team-anthropic',
    };
    apiMocks.putProvider.mockResolvedValue({
      configured: true,
      provider: created,
    });
    const onProviderSaved = vi.fn();
    render(
      <I18nProvider initialLocale="en">
        <SettingsModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId=""
          capabilityMode={null}
          onClose={vi.fn()}
          onSaveAuthKey={vi.fn()}
          onSaveProfileId={vi.fn()}
          onSaveAgentProfileId={vi.fn()}
          onSaveCapabilityMode={vi.fn()}
          onProviderSaved={onProviderSaved}
          onConfigured={vi.fn()}
        />
      </I18nProvider>,
    );

    await waitFor(() => {
      expect(screen.getByDisplayValue('test-model')).toBeTruthy();
    });
    fireEvent.click(screen.getByRole('button', { name: 'Add Provider' }));
    fireEvent.change(screen.getByLabelText('Profile id'), {
      target: { value: 'team-anthropic' },
    });
    fireEvent.change(screen.getByLabelText('Base URL'), {
      target: { value: 'https://anthropic.example.test' },
    });
    fireEvent.change(screen.getByLabelText('Provider adapter'), {
      target: { value: 'anthropic' },
    });
    fireEvent.change(screen.getByLabelText('API key'), {
      target: { value: 'provider-secret' },
    });
    fireEvent.change(screen.getByLabelText('Model'), {
      target: { value: 'claude-test' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => {
      expect(apiMocks.putProvider).toHaveBeenCalledWith(
        'daemon-key',
        'team-anthropic',
        expect.objectContaining({
          provider: 'anthropic',
          api_key: 'provider-secret',
          model: 'claude-test',
        }),
      );
      expect(onProviderSaved).toHaveBeenCalledWith(created);
    });
    expect(screen.getByRole('button', { name: /team-anthropic/ })).toBeTruthy();
  });

  it('attaches an MCP Profile to an Agent Profile', async () => {
    const savedAgent = {
      ...agentProfile,
      revision: 2,
      mcp_profile_ids: ['remote'],
    };
    apiMocks.putAgentProfile.mockResolvedValue({
      configured: true,
      agent_profile: savedAgent,
    });
    render(
      <I18nProvider initialLocale="en">
        <SettingsModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId=""
          capabilityMode={null}
          onClose={vi.fn()}
          onSaveAuthKey={vi.fn()}
          onSaveProfileId={vi.fn()}
          onSaveAgentProfileId={vi.fn()}
          onSaveCapabilityMode={vi.fn()}
          onProviderSaved={vi.fn()}
          onConfigured={vi.fn()}
        />
      </I18nProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Agent Profiles' }));
    await waitFor(() => {
      expect(screen.getByDisplayValue('Review carefully.')).toBeTruthy();
    });
    fireEvent.click(screen.getByLabelText(/remote/));
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => {
      expect(apiMocks.putAgentProfile).toHaveBeenCalledWith(
        'daemon-key',
        'reviewer',
        expect.objectContaining({
          mcp_profile_ids: ['remote'],
          initial_capability_mode: 'full_access',
        }),
      );
    });
  });

  it('creates a Streamable HTTP MCP Profile with credentials', async () => {
    apiMocks.listMcpProfiles.mockResolvedValue({ mcp_profiles: [] });
    const created = {
      ...remoteMcpProfile,
      transport: {
        ...remoteMcpProfile.transport,
        bearer_token_configured: true,
        header_names: ['x-api-key'],
      },
    };
    apiMocks.putMcpProfile.mockResolvedValue({
      configured: true,
      mcp_profile: created,
    });
    render(
      <I18nProvider initialLocale="en">
        <SettingsModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId=""
          capabilityMode={null}
          onClose={vi.fn()}
          onSaveAuthKey={vi.fn()}
          onSaveProfileId={vi.fn()}
          onSaveAgentProfileId={vi.fn()}
          onSaveCapabilityMode={vi.fn()}
          onProviderSaved={vi.fn()}
          onConfigured={vi.fn()}
        />
      </I18nProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'MCP Profiles' }));
    await waitFor(() => {
      expect(apiMocks.listMcpProfiles).toHaveBeenCalledWith('daemon-key');
    });
    fireEvent.change(screen.getByLabelText('MCP Profile id'), {
      target: { value: 'remote' },
    });
    fireEvent.change(screen.getByLabelText('Endpoint URL'), {
      target: { value: 'https://mcp.example.test/rpc' },
    });
    fireEvent.change(screen.getByLabelText('Bearer token (optional)'), {
      target: { value: 'bearer-secret' },
    });
    fireEvent.change(screen.getByLabelText(/Additional headers/), {
      target: { value: 'X-API-Key: header-secret' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => {
      expect(apiMocks.putMcpProfile).toHaveBeenCalledWith(
        'daemon-key',
        'remote',
        expect.objectContaining({
          transport: {
            type: 'http',
            url: 'https://mcp.example.test/rpc',
            bearer_token: 'bearer-secret',
            headers: { 'X-API-Key': 'header-secret' },
            allow_stateless: true,
            reinitialize_on_expired_session: true,
          },
        }),
      );
    });
  });

  it('creates a local stdio MCP Profile', async () => {
    apiMocks.listMcpProfiles.mockResolvedValue({ mcp_profiles: [] });
    const created: PublicMcpProfile = {
      ...remoteMcpProfile,
      mcp_profile_id: 'local',
      tool_name_prefix: 'local',
      transport: {
        type: 'stdio',
        command: 'npx',
        args: ['-y', '@example/mcp'],
        current_dir: null,
        env_keys: ['MCP_TOKEN'],
        clear_env: true,
      },
    };
    apiMocks.putMcpProfile.mockResolvedValue({
      configured: true,
      mcp_profile: created,
    });
    render(
      <I18nProvider initialLocale="en">
        <SettingsModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId=""
          capabilityMode={null}
          onClose={vi.fn()}
          onSaveAuthKey={vi.fn()}
          onSaveProfileId={vi.fn()}
          onSaveAgentProfileId={vi.fn()}
          onSaveCapabilityMode={vi.fn()}
          onProviderSaved={vi.fn()}
          onConfigured={vi.fn()}
        />
      </I18nProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'MCP Profiles' }));
    await waitFor(() => {
      expect(apiMocks.listMcpProfiles).toHaveBeenCalledWith('daemon-key');
    });
    fireEvent.change(screen.getByLabelText('MCP Profile id'), {
      target: { value: 'local' },
    });
    fireEvent.click(screen.getByRole('tab', { name: 'stdio process' }));
    fireEvent.change(screen.getByLabelText('Command'), {
      target: { value: 'npx' },
    });
    fireEvent.change(screen.getByLabelText(/Arguments/), {
      target: { value: '-y\n@example/mcp' },
    });
    fireEvent.change(screen.getByLabelText(/Environment variables/), {
      target: { value: 'MCP_TOKEN=stdio-secret' },
    });
    fireEvent.click(
      screen.getByLabelText('Clear the inherited process environment'),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => {
      expect(apiMocks.putMcpProfile).toHaveBeenCalledWith(
        'daemon-key',
        'local',
        expect.objectContaining({
          transport: {
            type: 'stdio',
            command: 'npx',
            args: ['-y', '@example/mcp'],
            current_dir: null,
            env: { MCP_TOKEN: 'stdio-secret' },
            clear_env: true,
          },
        }),
      );
    });
  });

  it('configures a Telegram bot account and recipient target, then tests delivery', async () => {
    const createdBot: PublicBotAccount = {
      type: 'telegram',
      bot_account_id: 'primary',
      revision: 1,
      bot_token_configured: true,
    };
    const createdTarget: PublicOutputChannel = {
      type: 'telegram',
      output_channel_id: 'alerts',
      revision: 1,
      bot_account_id: 'primary',
      bot_token_configured: true,
      chat_id: '-1001234567890',
    };
    apiMocks.putBotAccount.mockResolvedValue({
      configured: true,
      bot_account: createdBot,
    });
    apiMocks.putOutputChannel.mockResolvedValue({
      configured: true,
      output_channel: createdTarget,
    });
    apiMocks.testOutputChannel.mockResolvedValue(undefined);
    render(
      <I18nProvider initialLocale="en">
        <SettingsModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId=""
          capabilityMode={null}
          onClose={vi.fn()}
          onSaveAuthKey={vi.fn()}
          onSaveProfileId={vi.fn()}
          onSaveAgentProfileId={vi.fn()}
          onSaveCapabilityMode={vi.fn()}
          onProviderSaved={vi.fn()}
          onConfigured={vi.fn()}
        />
      </I18nProvider>,
    );

    fireEvent.click(screen.getByRole('button', { name: 'Telegram delivery' }));
    await waitFor(() => {
      expect(apiMocks.listBotAccounts).toHaveBeenCalledWith('daemon-key');
      expect(apiMocks.listOutputChannels).toHaveBeenCalledWith('daemon-key');
    });
    fireEvent.change(screen.getByLabelText('Bot account id'), {
      target: { value: 'primary' },
    });
    fireEvent.change(screen.getByLabelText(/Bot token/), {
      target: { value: '123456789:test_bot_token_with_enough_chars' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => {
      expect(apiMocks.putBotAccount).toHaveBeenCalledWith(
        'daemon-key',
        'primary',
        {
          type: 'telegram',
          bot_token: '123456789:test_bot_token_with_enough_chars',
        },
      );
    });

    fireEvent.click(screen.getByRole('tab', { name: 'Recipients' }));
    fireEvent.change(screen.getByLabelText('Recipient target id'), {
      target: { value: 'alerts' },
    });
    fireEvent.change(screen.getByLabelText(/Chat id/), {
      target: { value: '-1001234567890' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => {
      expect(apiMocks.putOutputChannel).toHaveBeenCalledWith(
        'daemon-key',
        'alerts',
        {
          type: 'telegram',
          bot_account_id: 'primary',
          chat_id: '-1001234567890',
        },
      );
    });
    fireEvent.click(screen.getByRole('button', { name: 'Send test' }));
    await waitFor(() => {
      expect(apiMocks.testOutputChannel).toHaveBeenCalledWith(
        'daemon-key',
        'alerts',
      );
    });
  });
});
