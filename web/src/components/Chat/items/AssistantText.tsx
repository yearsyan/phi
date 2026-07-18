import { memo, useState } from 'react';
import { useI18n } from '../../../i18n/I18nProvider.tsx';
import type { ForkPosition } from '../../../types/wire.ts';
import { CheckIcon, CopyIcon, ForkIcon } from '../../common/Icons.tsx';
import { Markdown } from '../../common/Markdown.tsx';
import styles from './AssistantText.module.css';
import { writeClipboard } from './clipboard.ts';
import { ReasoningBlock } from './ReasoningBlock.tsx';

interface AssistantTextProps {
  messageIndex?: number | null;
  forkPosition?: ForkPosition;
  reasoning?: string;
  text: string;
  streaming: boolean;
  forkEnabled?: boolean;
  onFork?: (messageIndex: number, position: ForkPosition) => Promise<void>;
}

/**
 * Assistant text row: a small status dot followed by markdown, no avatar and
 * no bubble. An empty streaming row renders the "Thinking…" placeholder.
 */
export const AssistantText = memo(function AssistantText({
  messageIndex = null,
  forkPosition = 'after',
  reasoning = '',
  text,
  streaming,
  forkEnabled = false,
  onFork,
}: AssistantTextProps) {
  const { t } = useI18n();
  const [copyState, setCopyState] = useState<'idle' | 'copied' | 'failed'>(
    'idle',
  );
  const [forkState, setForkState] = useState<'idle' | 'loading' | 'failed'>(
    'idle',
  );

  const copy = async () => {
    try {
      await writeClipboard(text);
      setCopyState('copied');
    } catch {
      setCopyState('failed');
    }
  };

  const fork = async () => {
    if (messageIndex === null || onFork === undefined || !forkEnabled) return;
    setForkState('loading');
    try {
      await onFork(messageIndex, forkPosition);
      setForkState('idle');
    } catch {
      setForkState('failed');
    }
  };

  const feedback =
    copyState === 'failed'
      ? t('chat.action.copyFailed')
      : forkState === 'failed'
        ? t('chat.action.forkFailed')
        : null;
  const forkLabel =
    forkPosition === 'before_tool_calls'
      ? t('chat.action.forkBeforeTools')
      : t('chat.action.fork');
  const showCopy = !streaming && text.length > 0;
  const showFork = messageIndex !== null && onFork !== undefined;

  return (
    <div className={styles.row}>
      <span
        className={`${styles.dot} ${streaming ? styles.dotLive : ''}`}
        aria-hidden="true"
      />
      <div className={styles.body}>
        {reasoning.length > 0 && (
          <ReasoningBlock reasoning={reasoning} streaming={streaming} />
        )}
        {text.length > 0 ? (
          <Markdown>{text}</Markdown>
        ) : streaming && reasoning.length === 0 ? (
          <span className={styles.thinking}>
            {t('chat.thinking')}
            <span className={styles.thinkingDots} aria-hidden="true">
              <i />
              <i />
              <i />
            </span>
          </span>
        ) : null}
        {text.length > 0 && (showCopy || showFork) && (
          <fieldset
            className={styles.actions}
            aria-label={t('chat.action.group')}
          >
            {showCopy && (
              <button
                type="button"
                className={`${styles.actionButton} ${
                  copyState === 'copied' ? styles.actionSuccess : ''
                }`}
                onClick={() => void copy()}
                aria-label={
                  copyState === 'copied'
                    ? t('chat.action.copied')
                    : t('chat.action.copy')
                }
                title={
                  copyState === 'copied'
                    ? t('chat.action.copied')
                    : t('chat.action.copy')
                }
              >
                {copyState === 'copied' ? <CheckIcon /> : <CopyIcon />}
              </button>
            )}
            {showFork && (
              <button
                type="button"
                className={`${styles.actionButton} ${
                  forkState === 'loading' ? styles.actionLoading : ''
                }`}
                onClick={() => void fork()}
                disabled={!forkEnabled || forkState === 'loading'}
                aria-label={forkLabel}
                title={
                  forkEnabled ? forkLabel : t('chat.action.forkUnavailable')
                }
              >
                <ForkIcon />
              </button>
            )}
            {feedback !== null && (
              <span className={styles.actionError} role="status">
                {feedback}
              </span>
            )}
          </fieldset>
        )}
      </div>
    </div>
  );
});
