import { useEffect, useRef, useState } from 'react';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import type { PlanApprovalRequest } from '../../types/wire.ts';
import { Markdown } from '../common/Markdown.tsx';
import styles from './PlanApprovalCard.module.css';

interface PlanApprovalCardProps {
  request: PlanApprovalRequest;
  onDecide: (
    approvalId: string,
    decision:
      | { type: 'approve'; revision: number }
      | { type: 'reject'; revision: number; feedback?: string | null },
  ) => boolean;
}

/**
 * Inline card for a `plan_approval_requested` interaction. Approves the exact
 * persisted revision, or rejects with optional feedback. The plan markdown is
 * rendered read-only; approval binds to the revision shown.
 */
export function PlanApprovalCard({ request, onDecide }: PlanApprovalCardProps) {
  const { t } = useI18n();
  const { approval_id, plan } = request;
  const [feedback, setFeedback] = useState('');
  const [mode, setMode] = useState<'idle' | 'reject'>('idle');
  const [submitting, setSubmitting] = useState(false);
  const retryTimer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (retryTimer.current !== null) {
        window.clearTimeout(retryTimer.current);
      }
    },
    [],
  );

  const markSubmitting = (sent: boolean) => {
    if (!sent) return;
    setSubmitting(true);
    retryTimer.current = window.setTimeout(() => {
      retryTimer.current = null;
      setSubmitting(false);
    }, 2_000);
  };

  const approve = () => {
    markSubmitting(
      onDecide(approval_id, {
        type: 'approve',
        revision: plan.revision,
      }),
    );
  };

  const reject = () => {
    markSubmitting(
      onDecide(approval_id, {
        type: 'reject',
        revision: plan.revision,
        feedback: feedback.trim().length > 0 ? feedback.trim() : null,
      }),
    );
  };

  return (
    <div className={styles.card}>
      <div className={styles.header}>
        <span className={styles.badge}>
          {t('plan.badge', { rev: plan.revision })}
        </span>
        <span className={styles.title}>{t('plan.title')}</span>
      </div>
      <div className={styles.plan}>
        <Markdown>{plan.content}</Markdown>
      </div>
      {mode === 'reject' && (
        <textarea
          className={styles.feedback}
          placeholder={t('plan.feedbackPlaceholder')}
          value={feedback}
          rows={3}
          onChange={(event) => setFeedback(event.target.value)}
        />
      )}
      <div className={styles.actions}>
        {mode === 'idle' ? (
          <>
            <button
              type="button"
              className={styles.rejectBtn}
              onClick={() => setMode('reject')}
              disabled={submitting}
            >
              {t('plan.requestChanges')}
            </button>
            <button
              type="button"
              className={styles.approveBtn}
              onClick={approve}
              disabled={submitting}
            >
              {t('plan.approve')}
            </button>
          </>
        ) : (
          <>
            <button
              type="button"
              className={styles.cancelBtn}
              onClick={() => setMode('idle')}
              disabled={submitting}
            >
              {t('plan.back')}
            </button>
            <button
              type="button"
              className={styles.rejectBtn}
              onClick={reject}
              disabled={submitting}
            >
              {submitting ? t('plan.submitting') : t('plan.sendFeedback')}
            </button>
          </>
        )}
      </div>
    </div>
  );
}
