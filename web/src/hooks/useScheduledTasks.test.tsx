/** @vitest-environment jsdom */

import { act, cleanup, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { ScheduledTask } from '../types/wire.ts';
import { useScheduledTasks } from './useScheduledTasks.ts';

const apiMocks = vi.hoisted(() => ({
  listScheduledTasks: vi.fn(),
  createScheduledTask: vi.fn(),
  updateScheduledTask: vi.fn(),
  runScheduledTask: vi.fn(),
  deleteScheduledTask: vi.fn(),
}));

vi.mock('../api/http.ts', () => apiMocks);

describe('useScheduledTasks', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    apiMocks.listScheduledTasks.mockResolvedValue({ tasks: [] });
    apiMocks.runScheduledTask.mockResolvedValue(undefined);
    apiMocks.deleteScheduledTask.mockResolvedValue(undefined);
  });

  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it('loads and refreshes while the scheduled-task page is active', async () => {
    vi.useFakeTimers();
    const first = task('task-1');
    apiMocks.listScheduledTasks
      .mockResolvedValueOnce({ tasks: [first] })
      .mockResolvedValueOnce({ tasks: [{ ...first, skipped_runs: 1 }] });

    const { result } = renderHook(() => useScheduledTasks('daemon-key', true));
    await act(async () => {
      await Promise.resolve();
    });
    expect(result.current.tasks).toEqual([first]);

    await act(async () => {
      vi.advanceTimersByTime(5_000);
      await Promise.resolve();
    });
    expect(apiMocks.listScheduledTasks).toHaveBeenCalledTimes(2);
    expect(result.current.tasks[0]?.skipped_runs).toBe(1);
  });

  it('creates, pauses, runs, and deletes through revision-aware requests', async () => {
    const created = task('task-1');
    const paused = {
      ...created,
      enabled: false,
      next_run_at: null,
      revision: 2,
    };
    const running = {
      ...paused,
      last_run: {
        scheduled_for: '2026-07-17T01:00:00Z',
        started_at: '2026-07-17T01:00:00Z',
        finished_at: null,
        outcome: 'running' as const,
        session_id: 'session-1',
        error: null,
      },
    };
    apiMocks.createScheduledTask.mockResolvedValue(created);
    apiMocks.updateScheduledTask.mockResolvedValue(paused);
    apiMocks.listScheduledTasks
      .mockResolvedValueOnce({ tasks: [] })
      .mockResolvedValueOnce({ tasks: [running] });

    const { result } = renderHook(() => useScheduledTasks('daemon-key', true));
    await act(async () => {
      await Promise.resolve();
    });

    await act(async () => {
      await result.current.createTask({
        name: created.name,
        prompt: created.prompt,
        schedule: created.schedule,
      });
    });
    expect(result.current.tasks).toEqual([created]);

    await act(async () => {
      await result.current.setEnabled(created, false);
    });
    expect(apiMocks.updateScheduledTask).toHaveBeenCalledWith(
      'daemon-key',
      'task-1',
      false,
      1,
    );
    expect(result.current.tasks).toEqual([paused]);

    await act(async () => {
      await result.current.runNow('task-1');
    });
    expect(apiMocks.runScheduledTask).toHaveBeenCalledWith(
      'daemon-key',
      'task-1',
    );
    expect(result.current.tasks).toEqual([running]);

    await act(async () => {
      await result.current.deleteTask('task-1');
    });
    expect(result.current.tasks).toEqual([]);
  });

  it('does not let polling hide a task while its create request is in flight', async () => {
    vi.useFakeTimers();
    const created = task('task-1');
    let resolveCreate: ((task: ScheduledTask) => void) | undefined;
    apiMocks.createScheduledTask.mockReturnValue(
      new Promise<ScheduledTask>((resolve) => {
        resolveCreate = resolve;
      }),
    );

    const { result } = renderHook(() => useScheduledTasks('daemon-key', true));
    await act(async () => {
      await Promise.resolve();
    });

    let create: Promise<ScheduledTask> | undefined;
    act(() => {
      create = result.current.createTask({
        name: created.name,
        prompt: created.prompt,
        schedule: created.schedule,
      });
    });
    await act(async () => {
      vi.advanceTimersByTime(5_000);
      await Promise.resolve();
    });
    expect(apiMocks.listScheduledTasks).toHaveBeenCalledTimes(1);

    await act(async () => {
      resolveCreate?.(created);
      await create;
    });
    expect(result.current.tasks).toEqual([created]);
  });
});

function task(taskId: string): ScheduledTask {
  return {
    task_id: taskId,
    name: 'Morning review',
    prompt: 'Review the workspace',
    workspace: '/workspace/phi',
    profile_id: 'default',
    agent_profile_id: 'default',
    capability_mode: null,
    schedule: {
      type: 'daily',
      time: '09:00',
      weekdays: ['monday', 'tuesday', 'wednesday', 'thursday', 'friday'],
      timezone: 'Asia/Singapore',
    },
    enabled: true,
    created_at: '2026-07-17T00:00:00Z',
    updated_at: '2026-07-17T00:00:00Z',
    next_run_at: '2026-07-17T01:00:00Z',
    last_run: null,
    skipped_runs: 0,
    revision: 1,
  };
}
