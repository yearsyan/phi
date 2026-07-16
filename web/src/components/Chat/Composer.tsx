import { useEffect, useRef, useState } from 'react';
import { useI18n } from '../../i18n/I18nProvider.tsx';
import { SendIcon, StopIcon } from '../common/Icons.tsx';
import styles from './Composer.module.css';

interface ComposerProps {
  disabled: boolean;
  busy: boolean;
  canStop: boolean;
  queuedCount: number;
  onSend: (text: string) => boolean;
  onStop: () => void;
}

export function Composer({
  disabled,
  busy,
  canStop,
  queuedCount,
  onSend,
  onStop,
}: ComposerProps) {
  const [value, setValue] = useState('');
  const { t } = useI18n();
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);

  // biome-ignore lint/correctness/useExhaustiveDependencies: value is the resize trigger; the effect only mutates the textarea ref
  useEffect(() => {
    const element = textareaRef.current;
    if (!element) return;
    element.style.height = 'auto';
    element.style.height = `${Math.min(element.scrollHeight, 220)}px`;
  }, [value]);

  const submit = () => {
    const prompt = value.trim();
    if (!prompt || disabled) return;
    if (onSend(prompt)) {
      setValue('');
      requestAnimationFrame(() => textareaRef.current?.focus());
    }
  };

  const onKeyDown = (event: React.KeyboardEvent<HTMLTextAreaElement>) => {
    if (
      event.key === 'Enter' &&
      !event.shiftKey &&
      !event.nativeEvent.isComposing
    ) {
      event.preventDefault();
      submit();
    }
  };

  const placeholder = disabled
    ? t('chat.composer.placeholderUnavailable')
    : busy
      ? t('chat.composer.placeholderQueue')
      : t('chat.composer.placeholder');

  return (
    <footer className={styles.dock}>
      <div
        className={`${styles.composer} ${disabled ? styles.composerDisabled : ''}`}
      >
        <textarea
          ref={textareaRef}
          className={styles.textarea}
          value={value}
          rows={1}
          disabled={disabled}
          placeholder={placeholder}
          aria-label={t('chat.composer.ariaLabel')}
          onChange={(event) => setValue(event.target.value)}
          onKeyDown={onKeyDown}
        />

        <div className={styles.footer}>
          <div className={styles.hints}>
            <span>{t('chat.composer.enter')}</span>
            <span>{t('chat.composer.shiftEnter')}</span>
            {queuedCount > 0 && (
              <span className={styles.queue}>
                {t('chat.composer.queued', { count: queuedCount })}
              </span>
            )}
          </div>
          <div className={styles.actions}>
            {canStop && (
              <button
                type="button"
                className={styles.stopButton}
                onClick={onStop}
                title={t('chat.composer.stopTitle')}
              >
                <StopIcon />
                <span>{t('chat.stop')}</span>
              </button>
            )}
            <button
              type="button"
              className={styles.sendButton}
              onClick={submit}
              disabled={disabled || value.trim().length === 0}
              title={t('chat.composer.sendTitle')}
            >
              <span>
                {busy ? t('chat.composer.queue') : t('chat.composer.send')}
              </span>
              <SendIcon />
            </button>
          </div>
        </div>
      </div>
      <p className={styles.disclaimer}>{t('chat.composer.disclaimer')}</p>
    </footer>
  );
}
