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
import type { PublicProviderConfig } from '../../types/wire.ts';
import { SettingsModal } from './SettingsModal.tsx';

const apiMocks = vi.hoisted(() => ({
  listProviders: vi.fn(),
  putProvider: vi.fn(),
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

describe('SettingsModal', () => {
  beforeEach(() => {
    apiMocks.listProviders.mockResolvedValue({ providers: [provider] });
    apiMocks.putProvider.mockReset();
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
});
