import { useEffect, useMemo, useState } from 'react';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type {
  RunActivity,
  Step,
  ToolStep,
} from '../../state/sessionReducer.ts';
import { ChevronIcon, TerminalIcon } from '../common/Icons.tsx';
import styles from './WorkDetail.module.css';

interface WorkDetailProps {
  run: RunActivity;
}

export function WorkDetail({ run }: WorkDetailProps) {
  const { t } = useI18n();
  const live = run.status === 'running' || run.status === 'queued';
  const [open, setOpen] = useState(live);
  const steps = useMemo(
    () =>
      run.turns.flatMap((turn) =>
        turn.steps.map((step) => ({ turn: turn.turn, step })),
      ),
    [run.turns],
  );

  // biome-ignore lint/correctness/useExhaustiveDependencies: a new run should reset the local disclosure state
  useEffect(() => {
    setOpen(live);
  }, [live, run.runId]);

  const toolCount = steps.filter(({ step }) => step.kind === 'tool').length;
  const summary = live
    ? toolCount > 0
      ? t('chat.activity.runningTools', { count: toolCount })
      : t('chat.activity.thinking')
    : run.status === 'failed'
      ? t('chat.activity.failed')
      : run.status === 'stopped'
        ? t('chat.activity.stopped')
        : t('chat.activity.completed', { count: toolCount });

  return (
    <section
      className={`${styles.activity} ${live ? styles.activityLive : ''}`}
    >
      <button
        type="button"
        className={styles.header}
        onClick={() => setOpen((current) => !current)}
        aria-expanded={open}
      >
        <span
          className={`${styles.runDot} ${styles[`run_${run.status}`] ?? ''}`}
        />
        <TerminalIcon />
        <span className={styles.summary}>{summary}</span>
        <span className={styles.runId}>{run.runId.slice(-6)}</span>
        <ChevronIcon
          className={`${styles.chevron} ${open ? styles.chevronOpen : ''}`}
        />
      </button>

      {open && (
        <div className={styles.timeline}>
          {steps.length === 0 ? (
            <div className={styles.waiting}>
              <span className={styles.waitingLine} />
              {t('chat.activity.waiting')}
            </div>
          ) : (
            steps.map(({ turn, step }, index) => (
              <StepRow
                key={stepKey(step, turn, index)}
                step={step}
                turn={turn}
              />
            ))
          )}
          {run.errorMessage && (
            <div className={`${styles.event} ${styles.eventError}`}>
              <span className={styles.eventRail} />
              <div>
                <div className={styles.eventLabel}>
                  {t('chat.activity.error')}
                </div>
                <div className={styles.eventText}>{run.errorMessage}</div>
              </div>
            </div>
          )}
        </div>
      )}
    </section>
  );
}

function StepRow({ step, turn }: { step: Step; turn: number }) {
  const { t } = useI18n();
  if (step.kind === 'tool') return <ToolRow step={step} turn={turn} />;

  const label =
    step.kind === 'retry'
      ? t('chat.activity.retry')
      : step.kind === 'compaction'
        ? t('chat.activity.compaction')
        : step.kind === 'subagent'
          ? t('chat.activity.subagent')
          : step.level;
  const text =
    step.kind === 'retry'
      ? `#${step.retryNumber}/${step.maxRetries} · ${step.reason}`
      : step.kind === 'compaction'
        ? (step.message ?? step.phase)
        : step.kind === 'subagent'
          ? `${step.message}${step.detail ? ` · ${step.detail}` : ''}`
          : step.message;

  return (
    <div
      className={`${styles.event} ${
        step.kind === 'compaction' && step.phase === 'failed'
          ? styles.eventError
          : ''
      }`}
    >
      <span className={styles.eventRail} />
      <div className={styles.eventBody}>
        <div className={styles.eventTop}>
          <span className={styles.eventLabel}>{label}</span>
          <span className={styles.turnLabel}>T{turn}</span>
        </div>
        <div className={styles.eventText}>{text}</div>
      </div>
    </div>
  );
}

function ToolRow({ step, turn }: { step: ToolStep; turn: number }) {
  const { t } = useI18n();
  const output = step.content?.trim() ?? '';
  const args = formatArguments(step.call.arguments);
  const progress = step.progress[step.progress.length - 1];

  return (
    <div className={styles.event}>
      <span className={styles.eventRail} />
      <div className={styles.eventBody}>
        <div className={styles.eventTop}>
          <span
            className={`${styles.toolState} ${
              step.status === 'running'
                ? styles.toolRunning
                : step.isError
                  ? styles.toolError
                  : styles.toolDone
            }`}
          />
          <span className={styles.toolName}>{step.call.name}</span>
          <span className={styles.turnLabel}>T{turn}</span>
        </div>
        <div className={styles.toolSummary}>
          {progress ??
            (step.status === 'running'
              ? t('chat.activity.toolRunning')
              : step.isError
                ? t('chat.activity.toolFailed')
                : t('chat.activity.toolDone'))}
        </div>
        {(args || output) && (
          <details className={styles.details}>
            <summary>{t('chat.activity.details')}</summary>
            {args && (
              <div className={styles.detailBlock}>
                <span>{t('chat.activity.arguments')}</span>
                <pre>{args}</pre>
              </div>
            )}
            {output && (
              <div className={styles.detailBlock}>
                <span>{t('chat.activity.output')}</span>
                <pre>{output}</pre>
              </div>
            )}
          </details>
        )}
      </div>
    </div>
  );
}

function stepKey(step: Step, turn: number, index: number): string {
  if (step.kind === 'tool') return `${turn}-${step.key}`;
  if (step.kind === 'retry')
    return `${turn}-retry-${step.retryNumber}-${index}`;
  if (step.kind === 'subagent') {
    return `${turn}-subagent-${step.agentId}-${index}`;
  }
  if (step.kind === 'compaction') {
    return `${turn}-compact-${step.phase}-${index}`;
  }
  return `${turn}-notice-${index}`;
}

function formatArguments(value: unknown): string {
  if (value === null || value === undefined) return '';
  try {
    return JSON.stringify(value, null, 2);
  } catch {
    return String(value);
  }
}
