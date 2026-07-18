import { memo } from 'react';
import { useI18n } from '../../../i18n/I18nProvider.tsx';
import type { CompactionMarker } from '../../../state/sessionReducer.ts';
import styles from './CompactionDivider.module.css';

interface CompactionDividerProps {
  phase: CompactionMarker['phase'];
  message?: string;
}

/** An indeterminate progress row that becomes a durable context boundary. */
export const CompactionDivider = memo(function CompactionDivider({
  phase,
  message,
}: CompactionDividerProps) {
  const { t } = useI18n();
  const text =
    phase === 'started'
      ? t('chat.statusLine.compaction.started')
      : phase === 'completed'
        ? t('chat.statusLine.compaction.completed')
        : message
          ? `${t('chat.statusLine.compaction.failed')}: ${message}`
          : t('chat.statusLine.compaction.failed');

  return (
    <div
      className={`${styles.divider} ${phase === 'failed' ? styles.failed : ''}`}
      role="status"
      aria-live="polite"
    >
      <span className={styles.line} />
      <span className={styles.label}>
        {phase === 'started' && <i className={styles.spinner} />}
        {text}
      </span>
      <span className={styles.line} />
    </div>
  );
});
