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
  getProvider: vi.fn(),
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

describe('SettingsModal', () => {
  beforeEach(() => {
    apiMocks.listProviders.mockResolvedValue({ providers: [provider] });
    apiMocks.getProvider.mockResolvedValue({
      configured: true,
      provider,
    });
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
          onConfigured={vi.fn()}
        />
      </I18nProvider>,
    );
    await waitFor(() => {
      expect(screen.getByDisplayValue('test-model')).toBeTruthy();
    });

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
});
