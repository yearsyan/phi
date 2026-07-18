import { useCallback, useEffect, useRef, useState } from 'react';
import {
  createScheduledTask as createScheduledTaskRequest,
  deleteScheduledTask as deleteScheduledTaskRequest,
  listScheduledTasks,
  runScheduledTask as runScheduledTaskRequest,
  updateScheduledTask as updateScheduledTaskRequest,
} from '../api/http.ts';
import type {
  CreateScheduledTaskRequest,
  ScheduledTask,
} from '../types/wire.ts';

const REFRESH_INTERVAL_MS = 5_000;

export interface ScheduledTasksState {
  tasks: ScheduledTask[];
  loading: boolean;
  error: string | null;
  refresh: () => Promise<void>;
  createTask: (request: CreateScheduledTaskRequest) => Promise<ScheduledTask>;
  setEnabled: (task: ScheduledTask, enabled: boolean) => Promise<ScheduledTask>;
  runNow: (taskId: string) => Promise<void>;
  deleteTask: (taskId: string) => Promise<void>;
}

export function useScheduledTasks(
  authKey: string,
  enabled: boolean,
): ScheduledTasksState {
  const [tasks, setTasks] = useState<ScheduledTask[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const authKeyRef = useRef(authKey);
  const tasksRef = useRef(tasks);
  const requestRevisionRef = useRef(0);
  const mutationsInFlightRef = useRef(0);
  authKeyRef.current = authKey;
  tasksRef.current = tasks;

  const refresh = useCallback(async () => {
    if (mutationsInFlightRef.current > 0) return;
    const revision = ++requestRevisionRef.current;
    const key = authKeyRef.current;
    if (!key) {
      setTasks([]);
      setError(null);
      return;
    }
    if (tasksRef.current.length === 0) setLoading(true);
    try {
      const response = await listScheduledTasks(key);
      if (revision !== requestRevisionRef.current) return;
      setTasks(response.tasks);
      setError(null);
    } catch (loadError) {
      if (revision !== requestRevisionRef.current) return;
      setError(
        loadError instanceof Error ? loadError.message : String(loadError),
      );
    } finally {
      if (revision === requestRevisionRef.current) setLoading(false);
    }
  }, []);

  const beginMutation = useCallback(() => {
    mutationsInFlightRef.current += 1;
    requestRevisionRef.current += 1;
    setLoading(false);
  }, []);

  const endMutation = useCallback(() => {
    mutationsInFlightRef.current = Math.max(
      0,
      mutationsInFlightRef.current - 1,
    );
  }, []);

  useEffect(() => {
    if (!enabled) {
      requestRevisionRef.current += 1;
      setTasks([]);
      setError(null);
      setLoading(false);
      return;
    }
    void refresh();
    const interval = window.setInterval(
      () => void refresh(),
      REFRESH_INTERVAL_MS,
    );
    return () => {
      window.clearInterval(interval);
      requestRevisionRef.current += 1;
    };
  }, [enabled, refresh]);

  const createTask = useCallback(
    async (request: CreateScheduledTaskRequest) => {
      beginMutation();
      try {
        const task = await createScheduledTaskRequest(
          authKeyRef.current,
          request,
        );
        setTasks((current) => [
          task,
          ...current.filter(
            (currentTask) => currentTask.task_id !== task.task_id,
          ),
        ]);
        setError(null);
        return task;
      } catch (createError) {
        setError(
          createError instanceof Error
            ? createError.message
            : String(createError),
        );
        throw createError;
      } finally {
        endMutation();
      }
    },
    [beginMutation, endMutation],
  );

  const setEnabled = useCallback(
    async (task: ScheduledTask, nextEnabled: boolean) => {
      beginMutation();
      try {
        const updated = await updateScheduledTaskRequest(
          authKeyRef.current,
          task.task_id,
          nextEnabled,
          task.revision,
        );
        setTasks((current) => replaceTask(current, updated));
        setError(null);
        return updated;
      } catch (updateError) {
        setError(
          updateError instanceof Error
            ? updateError.message
            : String(updateError),
        );
        throw updateError;
      } finally {
        endMutation();
      }
    },
    [beginMutation, endMutation],
  );

  const runNow = useCallback(
    async (taskId: string) => {
      beginMutation();
      try {
        await runScheduledTaskRequest(authKeyRef.current, taskId);
        const response = await listScheduledTasks(authKeyRef.current);
        setTasks(response.tasks);
        setError(null);
      } catch (runError) {
        setError(
          runError instanceof Error ? runError.message : String(runError),
        );
        throw runError;
      } finally {
        endMutation();
      }
    },
    [beginMutation, endMutation],
  );

  const deleteTask = useCallback(
    async (taskId: string) => {
      beginMutation();
      try {
        await deleteScheduledTaskRequest(authKeyRef.current, taskId);
        setTasks((current) =>
          current.filter((task) => task.task_id !== taskId),
        );
        setError(null);
      } catch (deleteError) {
        setError(
          deleteError instanceof Error
            ? deleteError.message
            : String(deleteError),
        );
        throw deleteError;
      } finally {
        endMutation();
      }
    },
    [beginMutation, endMutation],
  );

  return {
    tasks,
    loading,
    error,
    refresh,
    createTask,
    setEnabled,
    runNow,
    deleteTask,
  };
}

function replaceTask(
  tasks: readonly ScheduledTask[],
  replacement: ScheduledTask,
): ScheduledTask[] {
  return tasks.map((task) =>
    task.task_id === replacement.task_id ? replacement : task,
  );
}
