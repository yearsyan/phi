import { useEffect, useRef, useState } from 'react';
import { useScheduledTasks } from '../../hooks/useScheduledTasks.ts';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { CapabilityMode, ScheduledTask } from '../../types/wire.ts';
import {
  ClockIcon,
  MenuIcon,
  PauseIcon,
  PlayIcon,
  PlusIcon,
  TrashIcon,
} from '../common/Icons.tsx';
import { CreateScheduledTaskModal } from './CreateScheduledTaskModal.tsx';
import styles from './ScheduledTasksPage.module.css';

interface ScheduledTasksPageProps {
  authKey: string;
  profileId: string;
  agentProfileId: string;
  capabilityMode: CapabilityMode | null;
  onOpenSession: (sessionId: string) => void;
  onSessionsChanged: () => Promise<void>;
  onOpenSidebar: () => void;
}

export function ScheduledTasksPage({
  authKey,
  profileId,
  agentProfileId,
  capabilityMode,
  onOpenSession,
  onSessionsChanged,
  onOpenSidebar,
}: ScheduledTasksPageProps) {
  const { t, locale } = useI18n();
  const tasks = useScheduledTasks(authKey, true);
  const [createOpen, setCreateOpen] = useState(false);
  const [pendingTaskId, setPendingTaskId] = useState<string | null>(null);
  const observedSessions = useRef(new Set<string>());

  useEffect(() => {
    let changed = false;
    for (const task of tasks.tasks) {
      const sessionId = task.last_run?.session_id;
      if (sessionId && !observedSessions.current.has(sessionId)) {
        observedSessions.current.add(sessionId);
        changed = true;
      }
    }
    if (changed) void onSessionsChanged();
  }, [onSessionsChanged, tasks.tasks]);

  const runAction = async (taskId: string, action: () => Promise<unknown>) => {
    setPendingTaskId(taskId);
    try {
      await action();
    } catch {
      // The hook exposes the stable list-level error below the header.
    } finally {
      setPendingTaskId((current) => (current === taskId ? null : current));
    }
  };

  const deleteTask = (task: ScheduledTask) => {
    if (!window.confirm(t('scheduled.deleteConfirm', { name: task.name }))) {
      return;
    }
    void runAction(task.task_id, () => tasks.deleteTask(task.task_id));
  };

  const activeTasks = tasks.tasks.filter((task) => task.enabled);
  const pausedTasks = tasks.tasks.filter((task) => !task.enabled);

  return (
    <div className={styles.page}>
      <header className={styles.mobileHeader}>
        <button
          type="button"
          className={styles.mobileMenu}
          onClick={onOpenSidebar}
          aria-label={t('sidebar.sessions')}
        >
          <MenuIcon />
        </button>
        <span>{t('scheduled.title')}</span>
      </header>

      <div className={styles.content}>
        <div className={styles.headingRow}>
          <div>
            <h1>{t('scheduled.title')}</h1>
            <p>{t('scheduled.subtitle')}</p>
          </div>
          <button
            type="button"
            className={styles.createButton}
            onClick={() => setCreateOpen(true)}
          >
            <PlusIcon />
            <span>{t('scheduled.create')}</span>
          </button>
        </div>

        <ul className={styles.requirements}>
          <li>{t('scheduled.requirement.daemon')}</li>
          <li>{t('scheduled.requirement.workspace')}</li>
          <li>{t('scheduled.requirement.session')}</li>
        </ul>

        {tasks.error && (
          <div className={styles.error} role="alert">
            {tasks.error}
          </div>
        )}

        {tasks.loading && tasks.tasks.length === 0 ? (
          <div className={styles.loading}>{t('scheduled.loading')}</div>
        ) : tasks.tasks.length === 0 ? (
          <div className={styles.empty}>
            <span className={styles.emptyIcon}>
              <ClockIcon />
            </span>
            <h2>{t('scheduled.empty.title')}</h2>
            <p>{t('scheduled.empty.copy')}</p>
            <button type="button" onClick={() => setCreateOpen(true)}>
              {t('scheduled.create')}
            </button>
          </div>
        ) : (
          <div className={styles.groups}>
            {activeTasks.length > 0 && (
              <TaskGroup
                title={t('scheduled.active')}
                tasks={activeTasks}
                locale={locale}
                pendingTaskId={pendingTaskId}
                onOpenSession={onOpenSession}
                onToggle={(task) =>
                  void runAction(task.task_id, () =>
                    tasks.setEnabled(task, false),
                  )
                }
                onRun={(task) =>
                  void runAction(task.task_id, () => tasks.runNow(task.task_id))
                }
                onDelete={deleteTask}
              />
            )}
            {pausedTasks.length > 0 && (
              <TaskGroup
                title={t('scheduled.paused')}
                tasks={pausedTasks}
                locale={locale}
                pendingTaskId={pendingTaskId}
                onOpenSession={onOpenSession}
                onToggle={(task) =>
                  void runAction(task.task_id, () =>
                    tasks.setEnabled(task, true),
                  )
                }
                onRun={(task) =>
                  void runAction(task.task_id, () => tasks.runNow(task.task_id))
                }
                onDelete={deleteTask}
              />
            )}
          </div>
        )}
      </div>

      {createOpen && (
        <CreateScheduledTaskModal
          authKey={authKey}
          profileId={profileId}
          agentProfileId={agentProfileId}
          capabilityMode={capabilityMode}
          onClose={() => setCreateOpen(false)}
          onCreate={async (request) => {
            await tasks.createTask(request);
            setCreateOpen(false);
          }}
        />
      )}
    </div>
  );
}

interface TaskGroupProps {
  title: string;
  tasks: ScheduledTask[];
  locale: 'en' | 'zh';
  pendingTaskId: string | null;
  onOpenSession: (sessionId: string) => void;
  onToggle: (task: ScheduledTask) => void;
  onRun: (task: ScheduledTask) => void;
  onDelete: (task: ScheduledTask) => void;
}

function TaskGroup({
  title,
  tasks,
  locale,
  pendingTaskId,
  onOpenSession,
  onToggle,
  onRun,
  onDelete,
}: TaskGroupProps) {
  const { t } = useI18n();
  return (
    <section className={styles.group}>
      <h2>{title}</h2>
      <div className={styles.taskList}>
        {tasks.map((task) => {
          const pending = pendingTaskId === task.task_id;
          return (
            <article
              className={`${styles.taskCard} ${!task.enabled ? styles.taskPaused : ''}`}
              key={task.task_id}
              aria-busy={pending || undefined}
            >
              <button
                type="button"
                className={`${styles.statusToggle} ${task.enabled ? styles.statusEnabled : ''}`}
                onClick={() => onToggle(task)}
                disabled={pending}
                aria-label={
                  task.enabled
                    ? t('scheduled.action.pause')
                    : t('scheduled.action.resume')
                }
              >
                <span />
              </button>

              <div className={styles.taskBody}>
                <div className={styles.taskTitleRow}>
                  <h3>{task.name}</h3>
                  <span className={styles.scheduleBadge}>
                    <ClockIcon />
                    {formatSchedule(task, locale, t)}
                  </span>
                </div>
                <p className={styles.prompt}>{task.prompt}</p>
                <div className={styles.metadata}>
                  <span title={task.workspace}>{task.workspace}</span>
                  <span>
                    {task.next_run_at
                      ? t('scheduled.nextRun', {
                          time: formatDate(task.next_run_at, locale),
                        })
                      : t('scheduled.noNextRun')}
                  </span>
                  {task.last_run && (
                    <span className={outcomeClass(task, styles)}>
                      {outcomeLabel(task, t)}
                    </span>
                  )}
                </div>
                {task.last_run?.error && (
                  <div className={styles.runError} title={task.last_run.error}>
                    {task.last_run.error}
                  </div>
                )}
              </div>

              <div className={styles.actions}>
                {task.last_run?.session_id && (
                  <button
                    type="button"
                    onClick={() =>
                      onOpenSession(task.last_run?.session_id ?? '')
                    }
                  >
                    {t('scheduled.action.openSession')}
                  </button>
                )}
                <button
                  type="button"
                  onClick={() => onRun(task)}
                  disabled={pending || task.last_run?.outcome === 'running'}
                >
                  <PlayIcon />
                  {t('scheduled.action.run')}
                </button>
                <button
                  type="button"
                  onClick={() => onToggle(task)}
                  disabled={pending}
                >
                  {task.enabled ? <PauseIcon /> : <PlayIcon />}
                  {task.enabled
                    ? t('scheduled.action.pause')
                    : t('scheduled.action.resume')}
                </button>
                <button
                  type="button"
                  className={styles.deleteAction}
                  onClick={() => onDelete(task)}
                  disabled={pending}
                  aria-label={t('scheduled.action.delete')}
                >
                  <TrashIcon />
                </button>
              </div>
            </article>
          );
        })}
      </div>
    </section>
  );
}

function formatSchedule(
  task: ScheduledTask,
  locale: 'en' | 'zh',
  t: ReturnType<typeof useI18n>['t'],
): string {
  if (task.schedule.type === 'interval') {
    return t('scheduled.schedule.interval', {
      every: task.schedule.every,
      unit: t(`scheduled.unit.${task.schedule.unit}`),
    });
  }
  const weekdays = task.schedule.weekdays
    .map((weekday) => t(`scheduled.weekday.${weekday}`))
    .join(locale === 'zh' ? '、' : ', ');
  return t('scheduled.schedule.daily', {
    time: task.schedule.time,
    weekdays,
  });
}

function formatDate(value: string, locale: 'en' | 'zh'): string {
  return new Intl.DateTimeFormat(locale === 'zh' ? 'zh-CN' : 'en', {
    month: 'short',
    day: 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  }).format(new Date(value));
}

function outcomeLabel(
  task: ScheduledTask,
  t: ReturnType<typeof useI18n>['t'],
): string {
  const outcome = task.last_run?.outcome;
  if (!outcome) return '';
  return t(`scheduled.outcome.${outcome}`);
}

function outcomeClass(
  task: ScheduledTask,
  classes: Record<string, string>,
): string {
  switch (task.last_run?.outcome) {
    case 'succeeded':
      return classes.outcomeSuccess ?? '';
    case 'running':
      return classes.outcomeRunning ?? '';
    case 'failed':
    case 'interrupted':
      return classes.outcomeFailed ?? '';
    default:
      return '';
  }
}
