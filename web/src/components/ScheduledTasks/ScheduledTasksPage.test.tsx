/** @vitest-environment jsdom */

import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { I18nProvider } from '../../i18n/I18nProvider.tsx';
import type { ScheduledTask } from '../../types/wire.ts';
import { ScheduledTasksPage } from './ScheduledTasksPage.tsx';

const hookMocks = vi.hoisted(() => ({
  createTask: vi.fn(),
  setEnabled: vi.fn(),
  runNow: vi.fn(),
  deleteTask: vi.fn(),
  refresh: vi.fn(),
  tasks: [] as ScheduledTask[],
}));

vi.mock('../../hooks/useScheduledTasks.ts', () => ({
  useScheduledTasks: () => ({
    tasks: hookMocks.tasks,
    loading: false,
    error: null,
    refresh: hookMocks.refresh,
    createTask: hookMocks.createTask,
    setEnabled: hookMocks.setEnabled,
    runNow: hookMocks.runNow,
    deleteTask: hookMocks.deleteTask,
  }),
}));

describe('ScheduledTasksPage', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    hookMocks.setEnabled.mockResolvedValue(undefined);
    hookMocks.runNow.mockResolvedValue(undefined);
    hookMocks.deleteTask.mockResolvedValue(undefined);
    hookMocks.tasks = [task('active-task', true), task('paused-task', false)];
  });

  afterEach(() => {
    cleanup();
    vi.restoreAllMocks();
  });

  it('renders active and paused groups and routes task actions', async () => {
    const onOpenSession = vi.fn();
    renderPage(onOpenSession);

    expect(screen.getByRole('heading', { name: 'Active' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Paused' })).toBeTruthy();

    const activeCard = screen.getByText('active-task').closest('article');
    expect(activeCard).toBeTruthy();
    fireEvent.click(within(activeCard as HTMLElement).getByText('Run now'));
    await waitFor(() =>
      expect(hookMocks.runNow).toHaveBeenCalledWith('active-task'),
    );

    fireEvent.click(
      within(activeCard as HTMLElement).getByText('Open last session'),
    );
    expect(onOpenSession).toHaveBeenCalledWith('session-active-task');

    const pausedCard = screen.getByText('paused-task').closest('article');
    fireEvent.click(within(pausedCard as HTMLElement).getByText('Resume'));
    await waitFor(() =>
      expect(hookMocks.setEnabled).toHaveBeenCalledWith(
        expect.objectContaining({ task_id: 'paused-task' }),
        true,
      ),
    );
  });

  it('confirms deletion without stopping an already-created session', async () => {
    vi.spyOn(window, 'confirm').mockReturnValue(true);
    renderPage(vi.fn());
    const activeCard = screen.getByText('active-task').closest('article');
    fireEvent.click(
      within(activeCard as HTMLElement).getByRole('button', {
        name: 'Delete task',
      }),
    );
    await waitFor(() =>
      expect(hookMocks.deleteTask).toHaveBeenCalledWith('active-task'),
    );
  });
});

function renderPage(onOpenSession: (sessionId: string) => void) {
  return render(
    <I18nProvider initialLocale="en">
      <ScheduledTasksPage
        authKey="daemon-key"
        profileId="default"
        agentProfileId="default"
        capabilityMode={null}
        onOpenSession={onOpenSession}
        onSessionsChanged={vi.fn().mockResolvedValue(undefined)}
        onOpenSidebar={vi.fn()}
      />
    </I18nProvider>,
  );
}

function task(taskId: string, enabled: boolean): ScheduledTask {
  return {
    task_id: taskId,
    name: taskId,
    prompt: 'Review the workspace',
    workspace: '/workspace/phi',
    profile_id: 'default',
    agent_profile_id: 'default',
    capability_mode: null,
    schedule: {
      type: 'interval',
      every: 1,
      unit: 'hours',
    },
    enabled,
    created_at: '2026-07-17T00:00:00Z',
    updated_at: '2026-07-17T00:00:00Z',
    next_run_at: enabled ? '2026-07-17T01:00:00Z' : null,
    last_run: {
      scheduled_for: '2026-07-17T00:00:00Z',
      started_at: '2026-07-17T00:00:00Z',
      finished_at: '2026-07-17T00:01:00Z',
      outcome: 'succeeded',
      session_id: `session-${taskId}`,
      error: null,
    },
    skipped_runs: 0,
    revision: 1,
  };
}
