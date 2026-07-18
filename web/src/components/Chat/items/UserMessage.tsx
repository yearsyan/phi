import { memo, useState } from 'react';
import { useI18n } from '../../../i18n/I18nProvider.tsx';
import type { PendingPrompt } from '../../../state/sessionReducer.ts';
import type { Content, PublicMessage } from '../../../types/wire.ts';
import { CheckIcon, CopyIcon } from '../../common/Icons.tsx';
import { writeClipboard } from './clipboard.ts';
import styles from './UserMessage.module.css';

interface UserMessageProps {
  message: PublicMessage;
  pending: PendingPrompt | null;
}

/** Full-width rounded block, kimi-style: no avatar, no right-aligned bubble. */
export const UserMessage = memo(function UserMessage({
  message,
  pending,
}: UserMessageProps) {
  const { t } = useI18n();
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>(
    'idle',
  );
  const text = contentToText(message.content);

  const copy = async () => {
    try {
      await writeClipboard(text);
      setCopyState('copied');
    } catch {
      setCopyState('failed');
    }
  };

  return (
    <div className={styles.row}>
      <div className={styles.bubble}>{text}</div>
      {(text.length > 0 || pending !== null) && (
        <div className={styles.footer}>
          {text.length > 0 && (
            <fieldset
              className={styles.actions}
              aria-label={t('chat.action.messageGroup')}
            >
              <button
                type="button"
                className={`${styles.actionButton} ${
                  copyState === 'copied' ? styles.actionSuccess : ''
                }`}
                onClick={() => void copy()}
                aria-label={
                  copyState === 'copied'
                    ? t('chat.action.copied')
                    : t('chat.action.copyMessage')
                }
                title={
                  copyState === 'copied'
                    ? t('chat.action.copied')
                    : t('chat.action.copyMessage')
                }
              >
                {copyState === 'copied' ? <CheckIcon /> : <CopyIcon />}
              </button>
              {copyState === 'failed' && (
                <span className={styles.actionError} role="status">
                  {t('chat.action.copyFailed')}
                </span>
              )}
            </fieldset>
          )}
          {pending !== null && (
            <div className={styles.pending}>
              <span className={styles.pendingDot} />
              <span>
                {pending.queuePosition !== null
                  ? t('chat.prompt.queued', { position: pending.queuePosition })
                  : t('chat.prompt.sending')}
              </span>
            </div>
          )}
        </div>
      )}
    </div>
  );
});

export function contentToText(content: Content | null): string {
  if (content === null) return '';
  if (content.type === 'text') return content.value;
  return content.value
    .map((part) => {
      if (part.type === 'text') return part.text;
      if (part.type === 'document') return `[${part.document.filename}]`;
      return '[image]';
    })
    .join('');
}
