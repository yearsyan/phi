import { memo } from 'react';
import { useI18n } from '../../../i18n/I18nProvider.tsx';
import type { StatusStep } from '../../../state/timeline.ts';
import styles from './StatusLine.module.css';

interface StatusLineProps {
  step: StatusStep;
}

/** Small gray one-liner for run-level events (retry, notices…). */
export const StatusLine = memo(function StatusLine({ step }: StatusLineProps) {
  const { t } = useI18n();

  let tone: 'info' | 'warn' | 'error' = 'info';
  let text: string;
  switch (step.kind) {
    case 'notice':
      tone = step.level;
      text = step.message;
      break;
    case 'retry':
      tone = 'warn';
      text = t('chat.statusLine.retry', {
        n: step.retryNumber,
        max: step.maxRetries,
        reason: step.reason,
      });
      break;
    case 'subagent':
      text = step.detail ? `${step.message} — ${step.detail}` : step.message;
      break;
  }

  return (
    <div className={`${styles.row} ${styles[tone]}`} role="status">
      {text}
    </div>
  );
});
