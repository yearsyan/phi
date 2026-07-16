import { useEffect, useState } from 'react';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { TranslationKey } from '../../i18n/translations.ts';
import type {
  RunActivity,
  Step,
  ToolStep,
} from '../../state/sessionReducer.ts';
import { ChevronIcon, TerminalIcon } from '../common/Icons.tsx';
import styles from './WorkDetail.module.css';

interface WorkDetailProps {
  steps: Step[];
  collapsed: boolean;
  /** Live run status — used to render the header summary while running. */
  runStatus: RunActivity['status'] | null;
  turnNumber: number | null;
  errorMessage: string | null;
}

/**
 * Renders the per-turn "work detail" block: tool calls, retries, compaction
 * and subagent notices. Expanded while the turn is running; collapsed to a
 * single summary line once the turn ends. The user can always re-expand.
 */
export function WorkDetail({
  steps,
  collapsed,
  runStatus,
  turnNumber,
  errorMessage,
}: WorkDetailProps) {
  const { t } = useI18n();
  // The daemon drives `collapsed` (turn finished → collapse). The user can
  // override it locally afterwards.
  const [userOpen, setUserOpen] = useState<boolean | null>(null);
  // Reset the user override whenever the daemon changes the collapse state.
  // biome-ignore lint/correctness/useExhaustiveDependencies: collapsed is the trigger; body only resets state
  useEffect(() => {
    setUserOpen(null);
  }, [collapsed]);

  const open = userOpen ?? !collapsed;
  const summary = buildSummary(steps, runStatus, t);

  return (
    <div className={styles.wrap}>
      <button
        type="button"
        className={styles.header}
        onClick={() => setUserOpen(!open)}
        aria-expanded={open}
      >
        <ChevronIcon
          className={`${styles.chevron} ${open ? styles.chevronOpen : ''}`}
        />
        <TerminalIcon className={styles.icon} />
        <span className={styles.summary}>{summary}</span>
        {turnNumber !== null && (
          <span className={styles.badge}>
            {t('chat.workDetail.turn')} {turnNumber}
          </span>
        )}
        {errorMessage && (
          <span className={styles.errorFlag}>
            {t('chat.workDetail.failedFlag')}
          </span>
        )}
      </button>
      {open && steps.length > 0 && (
        <div className={styles.body}>
          {steps.map((step, index) => (
            <StepRow key={stepKey(step, index)} step={step} />
          ))}
        </div>
      )}
    </div>
  );
}

type TFunc = (
  key: TranslationKey,
  params?: Record<string, string | number>,
) => string;

function stepKey(step: Step, index: number): string {
  if (step.kind === 'tool') return `tool-${step.key}`;
  if (step.kind === 'subagent')
    return `subagent-${index}-${step.agentId}-${step.message}`;
  if (step.kind === 'retry') return `retry-${index}-${step.retryNumber}`;
  if (step.kind === 'compaction') return `compaction-${index}-${step.phase}`;
  return `notice-${index}-${step.message}`;
}

function toolWord(count: number, t: TFunc): string {
  return count === 1 ? t('chat.workDetail.tool') : t('chat.workDetail.tools');
}

function buildSummary(
  steps: Step[],
  runStatus: RunActivity['status'] | null,
  t: TFunc,
): string {
  const toolCount = steps.filter((step) => step.kind === 'tool').length;
  const running = steps.some(
    (step) => step.kind === 'tool' && step.status === 'running',
  );
  if (runStatus === 'running' || runStatus === 'queued') {
    if (running) {
      return toolCount > 0
        ? `${t('chat.workDetail.working')} · ${toolCount} ${toolWord(toolCount, t)}…`
        : `${t('chat.workDetail.working')}…`;
    }
    return toolCount > 0
      ? `${t('chat.workDetail.running')} ${toolCount} ${toolWord(toolCount, t)}…`
      : t('chat.workDetail.thinking');
  }
  if (runStatus === 'stopped') {
    return toolCount > 0
      ? `${t('chat.workDetail.stopped')} · ${toolCount} ${toolWord(toolCount, t)} ${t('chat.workDetail.beforeStop')}`
      : t('chat.workDetail.stopped');
  }
  if (runStatus === 'failed') {
    return toolCount > 0
      ? `${t('chat.workDetail.failed')} · ${t('chat.workDetail.after')} ${toolCount} ${toolWord(toolCount, t)}`
      : t('chat.workDetail.failed');
  }
  if (toolCount === 0) return t('chat.workDetail.completed');
  return `${t('chat.workDetail.ran')} ${toolCount} ${toolWord(toolCount, t)}`;
}

function StepRow({ step }: { step: Step }) {
  const { t } = useI18n();
  switch (step.kind) {
    case 'tool':
      return <ToolRow step={step} />;
    case 'retry':
      return (
        <div className={`${styles.step} ${styles.notice}`}>
          <span className={styles.stepLabel}>
            {t('chat.workDetail.failed')}
          </span>
          <span className={styles.stepText}>
            #{step.retryNumber}/{step.maxRetries} — {step.reason}
          </span>
        </div>
      );
    case 'compaction':
      return (
        <div className={`${styles.step} ${styles.notice}`}>
          <span className={styles.stepLabel}>{t('chat.toolResult')}</span>
          <span className={styles.stepText}>{step.message ?? step.phase}</span>
        </div>
      );
    case 'subagent':
      return (
        <div className={`${styles.step} ${styles.notice}`}>
          <span className={styles.stepLabel}>{t('chat.tool')}</span>
          <span className={styles.stepText}>
            {step.message}
            {step.detail ? (
              <span className={styles.stepDetail}> · {step.detail}</span>
            ) : null}
          </span>
        </div>
      );
    case 'notice':
      return (
        <div className={`${styles.step} ${styles.notice}`}>
          <span className={styles.stepLabel}>{step.level}</span>
          <span className={styles.stepText}>{step.message}</span>
        </div>
      );
    default:
      return null;
  }
}

function ToolRow({ step }: { step: ToolStep }) {
  const [expanded, setExpanded] = useState(false);
  const hasOutput = step.content !== null && step.content.length > 0;
  const truncatedArgs = truncate(JSON.stringify(step.call.arguments));
  return (
    <div className={`${styles.step} ${styles.tool}`}>
      <button
        type="button"
        className={styles.toolHead}
        onClick={() => hasOutput && setExpanded(!expanded)}
      >
        <span
          className={`${styles.statusDot} ${
            step.status === 'running'
              ? styles.statusRunning
              : step.isError
                ? styles.statusError
                : styles.statusOk
          }`}
        />
        <span className={styles.toolName}>{step.call.name}</span>
        <span className={styles.toolArgs}>{truncatedArgs}</span>
        {step.progress.length > 0 && step.status === 'running' && (
          <span className={styles.progressLast}>
            {step.progress[step.progress.length - 1]}
          </span>
        )}
      </button>
      {expanded && hasOutput && (
        <pre className={styles.output}>{step.content}</pre>
      )}
    </div>
  );
}

function truncate(value: string, max = 120): string {
  const compact = value.replace(/\s+/g, ' ').trim();
  return compact.length > max ? `${compact.slice(0, max)}…` : compact;
}
