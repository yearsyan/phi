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
import { CreateScheduledTaskModal } from './CreateScheduledTaskModal.tsx';

const apiMocks = vi.hoisted(() => ({
  browseWorkspace: vi.fn(),
  listProviders: vi.fn(),
  listAgentProfiles: vi.fn(),
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
  });

  afterEach(() => cleanup());

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
});
