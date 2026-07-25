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
import type { ScheduledTask } from '../../types/wire.ts';
import { CreateScheduledTaskModal } from './CreateScheduledTaskModal.tsx';

const apiMocks = vi.hoisted(() => ({
  browseWorkspace: vi.fn(),
  listProviders: vi.fn(),
  listAgentProfiles: vi.fn(),
  listOutputChannels: vi.fn(),
}));

vi.mock('../../api/http.ts', () => apiMocks);

describe('CreateScheduledTaskModal', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiMocks.browseWorkspace.mockResolvedValue({
      path: '/workspace/phi',
      parent: '/workspace',
      directories: [],
      truncated: false,
    });
    apiMocks.listProviders.mockResolvedValue({
      providers: [{ profile_id: 'default', model: 'test-model' }],
    });
    apiMocks.listAgentProfiles.mockResolvedValue({
      agent_profiles: [{ agent_profile_id: 'default' }],
    });
    apiMocks.listOutputChannels.mockResolvedValue({
      output_channels: [
        {
          type: 'telegram',
          output_channel_id: 'alerts',
          revision: 1,
          bot_account_id: 'primary',
          bot_token_configured: true,
          chat_id: '-1001234567890',
        },
      ],
    });
  });

  afterEach(() => cleanup());

  it('keeps prompt focus when callback props change', async () => {
    const firstOnClose = vi.fn();
    const { rerender } = render(
      <I18nProvider initialLocale="en">
        <CreateScheduledTaskModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId="default"
          capabilityMode={null}
          onClose={firstOnClose}
          onCreate={vi.fn()}
        />
      </I18nProvider>,
    );
    await screen.findByRole('option', { name: 'alerts · Telegram' });
    const prompt = screen.getByLabelText('Prompt');
    prompt.focus();
    expect(document.activeElement).toBe(prompt);

    rerender(
      <I18nProvider initialLocale="en">
        <CreateScheduledTaskModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId="default"
          capabilityMode={null}
          onClose={vi.fn()}
          onCreate={vi.fn()}
        />
      </I18nProvider>,
    );

    expect(document.activeElement).toBe(prompt);
  });

  it('prefills and replaces an existing task through its current revision', async () => {
    const onUpdate = vi.fn().mockResolvedValue(undefined);
    render(
      <I18nProvider initialLocale="en">
        <CreateScheduledTaskModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId="default"
          capabilityMode="full_access"
          task={existingTask()}
          onClose={vi.fn()}
          onCreate={vi.fn()}
          onUpdate={onUpdate}
        />
      </I18nProvider>,
    );
    await screen.findByRole('option', { name: 'alerts · Telegram' });
    expect(
      screen.getByRole('heading', { name: 'Edit scheduled task' }),
    ).toBeTruthy();
    expect((screen.getByLabelText('Name') as HTMLInputElement).value).toBe(
      'Existing review',
    );
    expect((screen.getByLabelText('Prompt') as HTMLTextAreaElement).value).toBe(
      'Review the existing workspace',
    );

    fireEvent.change(screen.getByLabelText('Prompt'), {
      target: { value: 'Review failures and suggest fixes' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Save' }));

    await waitFor(() => expect(onUpdate).toHaveBeenCalledTimes(1));
    expect(onUpdate).toHaveBeenCalledWith({
      name: 'Existing review',
      prompt: 'Review failures and suggest fixes',
      workspace: '/workspace/existing',
      profile_id: 'default',
      agent_profile_id: 'default',
      capability_mode: null,
      output_channel_id: 'alerts',
      schedule: {
        type: 'interval',
        every: 30,
        unit: 'minutes',
      },
      expected_revision: 7,
    });
  });

  it('creates the default weekday schedule with workspace and profile policy', async () => {
    const onCreate = vi.fn().mockResolvedValue(undefined);
    render(
      <I18nProvider initialLocale="en">
        <CreateScheduledTaskModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId="default"
          capabilityMode="workspace_edit"
          onClose={vi.fn()}
          onCreate={onCreate}
        />
      </I18nProvider>,
    );

    fireEvent.change(screen.getByLabelText('Name'), {
      target: { value: 'Morning review' },
    });
    fireEvent.change(screen.getByLabelText('Prompt'), {
      target: { value: 'Review the latest workspace changes' },
    });
    await waitFor(() =>
      expect(apiMocks.browseWorkspace).toHaveBeenCalledWith('daemon-key'),
    );
    fireEvent.click(screen.getByRole('button', { name: 'Create' }));

    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(1));
    expect(onCreate).toHaveBeenCalledWith(
      expect.objectContaining({
        name: 'Morning review',
        prompt: 'Review the latest workspace changes',
        workspace: '/workspace/phi',
        profile_id: 'default',
        agent_profile_id: 'default',
        capability_mode: 'workspace_edit',
        schedule: expect.objectContaining({
          type: 'daily',
          time: '09:00',
          weekdays: ['monday', 'tuesday', 'wednesday', 'thursday', 'friday'],
        }),
      }),
    );
  });

  it('shows the MCP profiles linked by the selected agent profile', async () => {
    apiMocks.listAgentProfiles.mockResolvedValue({
      agent_profiles: [
        {
          agent_profile_id: 'default',
          mcp_profile_ids: ['search', 'filesystem'],
        },
      ],
    });
    render(
      <I18nProvider initialLocale="en">
        <CreateScheduledTaskModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId="default"
          capabilityMode={null}
          onClose={vi.fn()}
          onCreate={vi.fn()}
        />
      </I18nProvider>,
    );

    await waitFor(() =>
      expect(screen.getByText('MCP: search, filesystem')).toBeTruthy(),
    );
  });

  it('explains that MCP tools come from the agent profile when none are linked', async () => {
    render(
      <I18nProvider initialLocale="en">
        <CreateScheduledTaskModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId="default"
          capabilityMode={null}
          onClose={vi.fn()}
          onCreate={vi.fn()}
        />
      </I18nProvider>,
    );

    await waitFor(() =>
      expect(
        screen.getByText(
          'No MCP linked. MCP tools come from the Agent Profile; configure them in Settings.',
        ),
      ).toBeTruthy(),
    );
  });

  it('switches to an interval schedule', async () => {
    const onCreate = vi.fn().mockResolvedValue(undefined);
    render(
      <I18nProvider initialLocale="en">
        <CreateScheduledTaskModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId="default"
          capabilityMode={null}
          onClose={vi.fn()}
          onCreate={onCreate}
        />
      </I18nProvider>,
    );
    fireEvent.change(screen.getByLabelText('Name'), {
      target: { value: 'Frequent check' },
    });
    fireEvent.change(screen.getByLabelText('Prompt'), {
      target: { value: 'Check status' },
    });
    fireEvent.click(screen.getByRole('tab', { name: 'Interval' }));
    const every = screen.getByRole('spinbutton');
    fireEvent.change(every, { target: { value: '30' } });
    fireEvent.change(screen.getByDisplayValue('hours'), {
      target: { value: 'minutes' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Create' }));

    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(1));
    expect(onCreate.mock.calls[0]?.[0].schedule).toEqual({
      type: 'interval',
      every: 30,
      unit: 'minutes',
    });
  });

  it('attaches the selected output channel', async () => {
    const onCreate = vi.fn().mockResolvedValue(undefined);
    render(
      <I18nProvider initialLocale="en">
        <CreateScheduledTaskModal
          authKey="daemon-key"
          profileId="default"
          agentProfileId="default"
          capabilityMode={null}
          onClose={vi.fn()}
          onCreate={onCreate}
        />
      </I18nProvider>,
    );

    fireEvent.change(screen.getByLabelText('Name'), {
      target: { value: 'Notify me' },
    });
    fireEvent.change(screen.getByLabelText('Prompt'), {
      target: { value: 'Check status' },
    });
    await waitFor(() =>
      expect(
        screen.getByRole('option', { name: 'alerts · Telegram' }),
      ).toBeTruthy(),
    );
    fireEvent.change(screen.getByLabelText('Recipient target'), {
      target: { value: 'alerts' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'Create' }));

    await waitFor(() => expect(onCreate).toHaveBeenCalledTimes(1));
    expect(onCreate.mock.calls[0]?.[0].output_channel_id).toBe('alerts');
  });
});

function existingTask(): ScheduledTask {
  return {
    task_id: 'task-1',
    name: 'Existing review',
    prompt: 'Review the existing workspace',
    workspace: '/workspace/existing',
    profile_id: 'default',
    agent_profile_id: 'default',
    capability_mode: null,
    output_channel_id: 'alerts',
    schedule: {
      type: 'interval',
      every: 30,
      unit: 'minutes',
    },
    enabled: true,
    created_at: '2026-07-25T00:00:00Z',
    updated_at: '2026-07-25T00:00:00Z',
    next_run_at: '2026-07-25T00:30:00Z',
    last_run: null,
    skipped_runs: 0,
    revision: 7,
  };
}
